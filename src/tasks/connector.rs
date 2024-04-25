use {
    crate::{
        backend::{Connector, ConnectorEvent, ConnectorId, MonitorInfo},
        globals::GlobalName,
        ifs::wl_output::{OutputId, PersistentOutputState, WlOutputGlobal},
        state::{ConnectorData, OutputData, State},
        tree::{move_ws_to_output, OutputNode, OutputRenderData, WsMoveConfig},
        utils::{asyncevent::AsyncEvent, clonecell::CloneCell},
    },
    std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        rc::Rc,
    },
};

pub fn handle(state: &Rc<State>, connector: &Rc<dyn Connector>) {
    let mut drm_dev = None;
    if let Some(dev_id) = connector.drm_dev() {
        drm_dev = match state.drm_devs.get(&dev_id) {
            Some(dev) => Some(dev),
            _ => panic!("connector's drm device does not exist"),
        };
    }
    let id = connector.id();
    let data = Rc::new(ConnectorData {
        connector: connector.clone(),
        handler: Default::default(),
        connected: Cell::new(false),
        name: connector.kernel_id().to_string(),
        drm_dev: drm_dev.clone(),
        async_event: Rc::new(AsyncEvent::default()),
    });
    if let Some(dev) = drm_dev {
        dev.connectors.set(id, data.clone());
    }
    let oh = ConnectorHandler {
        id,
        state: state.clone(),
        data: data.clone(),
    };
    let future = state.eng.spawn(oh.handle());
    data.handler.set(Some(future));
    if state.connectors.set(id, data).is_some() {
        panic!("Connector id has been reused");
    }
}

struct ConnectorHandler {
    id: ConnectorId,
    state: Rc<State>,
    data: Rc<ConnectorData>,
}

impl ConnectorHandler {
    async fn handle(self) {
        {
            let ae = self.data.async_event.clone();
            self.data.connector.on_change(Rc::new(move || ae.trigger()));
        }
        if let Some(config) = self.state.config.get() {
            config.new_connector(self.id);
        }
        'outer: loop {
            while let Some(event) = self.data.connector.event() {
                match event {
                    ConnectorEvent::Removed => break 'outer,
                    ConnectorEvent::Connected(mi) => self.handle_connected(mi).await,
                    _ => unreachable!(),
                }
            }
            self.data.async_event.triggered().await;
        }
        if let Some(dev) = &self.data.drm_dev {
            dev.connectors.remove(&self.id);
        }
        if let Some(config) = self.state.config.get() {
            config.del_connector(self.id);
        }
        self.data.handler.set(None);
        self.state.connectors.remove(&self.id);
    }

    async fn handle_connected(&self, info: MonitorInfo) {
        log::info!("Connector {} connected", self.data.connector.kernel_id());
        self.data.connected.set(true);
        let name = self.state.globals.name();
        let output_id = Rc::new(OutputId {
            connector: self.data.name.clone(),
            manufacturer: info.manufacturer.clone(),
            model: info.product.clone(),
            serial_number: info.serial_number.clone(),
        });
        if info.non_desktop {
            self.handle_non_desktop_connected(info).await;
        } else {
            self.handle_desktop_connected(info, name, output_id).await;
        }
        self.data.connected.set(false);
        log::info!("Connector {} disconnected", self.data.connector.kernel_id());
    }

    async fn handle_desktop_connected(
        &self,
        info: MonitorInfo,
        name: GlobalName,
        output_id: Rc<OutputId>,
    ) {
        let desired_state = match self.state.persistent_output_states.get(&output_id) {
            Some(ds) => ds,
            _ => {
                let x1 = self
                    .state
                    .root
                    .outputs
                    .lock()
                    .values()
                    .map(|o| o.global.pos.get().x2())
                    .max()
                    .unwrap_or(0);
                let ds = Rc::new(PersistentOutputState {
                    transform: Default::default(),
                    scale: Default::default(),
                    pos: Cell::new((x1, 0)),
                });
                self.state
                    .persistent_output_states
                    .set(output_id.clone(), ds.clone());
                ds
            }
        };
        let global = Rc::new(WlOutputGlobal::new(
            name,
            &self.state,
            &self.data,
            info.modes.clone(),
            &info.initial_mode,
            info.width_mm,
            info.height_mm,
            &output_id,
            &desired_state,
        ));
        let on = Rc::new(OutputNode {
            id: self.state.node_ids.next(),
            workspaces: Default::default(),
            workspace: CloneCell::new(None),
            seat_state: Default::default(),
            global: global.clone(),
            layers: Default::default(),
            render_data: RefCell::new(OutputRenderData {
                active_workspace: None,
                underline: Default::default(),
                inactive_workspaces: Default::default(),
                attention_requested_workspaces: Default::default(),
                captured_inactive_workspaces: Default::default(),
                titles: Default::default(),
                status: None,
            }),
            state: self.state.clone(),
            is_dummy: false,
            status: self.state.status.clone(),
            scroll: Default::default(),
            pointer_positions: Default::default(),
            lock_surface: Default::default(),
            hardware_cursor: Default::default(),
            jay_outputs: Default::default(),
            screencasts: Default::default(),
            update_render_data_scheduled: Cell::new(false),
            hardware_cursor_needs_render: Cell::new(false),
            screencopies: Default::default(),
        });
        self.state
            .add_output_scale(on.global.persistent.scale.get());
        let output_data = Rc::new(OutputData {
            connector: self.data.clone(),
            monitor_info: info,
            node: Some(on.clone()),
        });
        self.state.outputs.set(self.id, output_data);
        on.schedule_update_render_data();
        self.state.root.outputs.set(self.id, on.clone());
        self.state.root.update_extents();
        global.opt.node.set(Some(on.clone()));
        global.opt.global.set(Some(global.clone()));
        let mut ws_to_move = VecDeque::new();
        if self.state.root.outputs.len() == 1 {
            let seats = self.state.globals.seats.lock();
            let pos = global.pos.get();
            let x = (pos.x1() + pos.x2()) / 2;
            let y = (pos.y1() + pos.y2()) / 2;
            for seat in seats.values() {
                seat.set_position(x, y);
            }
            let dummy = self.state.dummy_output.get().unwrap();
            for ws in dummy.workspaces.iter() {
                if ws.is_dummy {
                    continue;
                }
                ws_to_move.push_back(ws);
            }
        }
        for source in self.state.root.outputs.lock().values() {
            if source.id == on.id {
                continue;
            }
            for ws in source.workspaces.iter() {
                if ws.is_dummy {
                    continue;
                }
                if ws.desired_output.get() == global.output_id {
                    ws_to_move.push_back(ws.clone());
                }
            }
        }
        while let Some(ws) = ws_to_move.pop_front() {
            let make_visible = (ws.visible_on_desired_output.get()
                && ws.desired_output.get() == output_id)
                || ws_to_move.is_empty();
            let config = WsMoveConfig {
                make_visible_if_empty: make_visible,
                source_is_destroyed: false,
            };
            move_ws_to_output(&ws, &on, config);
        }
        if let Some(config) = self.state.config.get() {
            config.connector_connected(self.id);
        }
        self.state.add_global(&global);
        self.state.tree_changed();
        'outer: loop {
            while let Some(event) = self.data.connector.event() {
                match event {
                    ConnectorEvent::Disconnected => break 'outer,
                    ConnectorEvent::HardwareCursor(hc) => {
                        on.hardware_cursor.set(hc);
                        self.state.refresh_hardware_cursors();
                    }
                    ConnectorEvent::ModeChanged(mode) => {
                        on.update_mode(mode);
                    }
                    ev => unreachable!("received unexpected event {:?}", ev),
                }
            }
            self.data.async_event.triggered().await;
        }
        if let Some(config) = self.state.config.get() {
            config.connector_disconnected(self.id);
        }
        global.clear();
        for (_, jo) in on.jay_outputs.lock().drain() {
            jo.send_destroyed();
        }
        let screencasts: Vec<_> = on.screencasts.lock().values().cloned().collect();
        for sc in screencasts {
            sc.do_destroy();
        }
        for (_, sc) in on.screencopies.lock().drain() {
            sc.send_failed();
        }
        global.destroyed.set(true);
        self.state.root.outputs.remove(&self.id);
        self.state.root.update_extents();
        self.state.outputs.remove(&self.id);
        on.lock_surface.take();
        {
            let mut surfaces = vec![];
            for layer in &on.layers {
                surfaces.extend(layer.iter());
            }
            for surface in surfaces {
                surface.destroy_node();
                surface.send_closed();
            }
        }
        let target = match self.state.root.outputs.lock().values().next() {
            Some(o) => o.clone(),
            _ => self.state.dummy_output.get().unwrap(),
        };
        for ws in on.workspaces.iter() {
            if ws.desired_output.get() == output_id {
                ws.visible_on_desired_output.set(ws.visible.get());
            }
            let config = WsMoveConfig {
                make_visible_if_empty: ws.visible.get(),
                source_is_destroyed: true,
            };
            move_ws_to_output(&ws, &target, config);
        }
        let seats = self.state.globals.seats.lock();
        for seat in seats.values() {
            if seat.get_output().id == on.id {
                let tpos = target.global.pos.get();
                seat.set_position((tpos.x1() + tpos.x2()) / 2, (tpos.y1() + tpos.y2()) / 2);
            }
        }
        if let Some(dev) = &self.data.drm_dev {
            dev.connectors.remove(&self.id);
        }
        self.state
            .remove_output_scale(on.global.persistent.scale.get());
        let _ = self.state.remove_global(&*global);
        self.state.tree_changed();
        self.state.damage();
    }

    async fn handle_non_desktop_connected(&self, monitor_info: MonitorInfo) {
        let output_data = Rc::new(OutputData {
            connector: self.data.clone(),
            monitor_info,
            node: None,
        });
        self.state.outputs.set(self.id, output_data);
        if let Some(config) = self.state.config.get() {
            config.connector_connected(self.id);
        }
        'outer: loop {
            while let Some(event) = self.data.connector.event() {
                match event {
                    ConnectorEvent::Disconnected => break 'outer,
                    ConnectorEvent::Available => {}
                    ConnectorEvent::Unavailable => {}
                    ev => unreachable!("received unexpected event {:?}", ev),
                }
            }
            self.data.async_event.triggered().await;
        }
        self.state.outputs.remove(&self.id);
        if let Some(config) = self.state.config.get() {
            config.connector_disconnected(self.id);
        }
    }
}
