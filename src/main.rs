#![feature(
    generic_associated_types,
    type_alias_impl_trait,
    never_type,
    c_variadic
)]

use crate::acceptor::AcceptorError;
use crate::async_engine::AsyncError;
use crate::backends::xorg::{XorgBackend, XorgBackendError};
use crate::client::Clients;
use crate::clientmem::ClientMemError;
use crate::event_loop::EventLoopError;
use crate::globals::Globals;
use crate::ifs::wl_compositor::WlCompositorGlobal;
use crate::ifs::wl_shm::WlShmGlobal;
use crate::ifs::wl_subcompositor::WlSubcompositorGlobal;
use crate::ifs::xdg_wm_base::XdgWmBaseGlobal;
use crate::sighand::SighandError;
use crate::state::State;
use crate::tree::{DisplayNode, NodeIds};
use crate::utils::numcell::NumCell;
use crate::utils::queue::AsyncQueue;
use crate::wheel::WheelError;
use acceptor::Acceptor;
use anyhow::anyhow;
use async_engine::AsyncEngine;
use event_loop::EventLoop;
use log::LevelFilter;
use std::rc::Rc;
use thiserror::Error;
use wheel::Wheel;

#[macro_use]
mod macros;
mod acceptor;
mod async_engine;
mod backend;
mod backends;
mod client;
mod clientmem;
mod event_loop;
mod format;
mod globals;
mod ifs;
mod object;
mod pixman;
mod servermem;
mod sighand;
mod state;
mod tasks;
mod time;
mod tree;
mod utils;
mod wheel;
mod xkbcommon;
mod fixed;

fn main() {
    env_logger::builder()
        .filter_level(LevelFilter::Trace)
        .init();
    if let Err(e) = main_() {
        log::error!("A fatal error occurred: {:#}", anyhow!(e));
        std::process::exit(1);
    }
}

#[derive(Debug, Error)]
enum MainError {
    #[error("The client acceptor caused an error")]
    AcceptorError(#[from] AcceptorError),
    #[error("The event loop caused an error")]
    EventLoopError(#[from] EventLoopError),
    #[error("The signal handler caused an error")]
    SighandError(#[from] SighandError),
    #[error("The clientmem subsystem caused an error")]
    ClientmemError(#[from] ClientMemError),
    #[error("The timer subsystem caused an error")]
    WheelError(#[from] WheelError),
    #[error("The async subsystem caused an error")]
    AsyncError(#[from] AsyncError),
    #[error("The xorg backend caused an error")]
    XorgBackendError(#[from] XorgBackendError),
}

fn main_() -> Result<(), MainError> {
    clientmem::init()?;
    let el = EventLoop::new()?;
    sighand::install(&el)?;
    let wheel = Wheel::install(&el)?;
    let engine = AsyncEngine::install(&el, &wheel)?;
    let globals = Globals::new();
    globals.insert_no_broadcast(Rc::new(WlCompositorGlobal::new(globals.name())));
    globals.insert_no_broadcast(Rc::new(WlShmGlobal::new(globals.name())));
    globals.insert_no_broadcast(Rc::new(WlSubcompositorGlobal::new(globals.name())));
    globals.insert_no_broadcast(Rc::new(XdgWmBaseGlobal::new(globals.name())));
    let node_ids = NodeIds::default();
    let state = Rc::new(State {
        eng: engine.clone(),
        el: el.clone(),
        wheel,
        clients: Clients::new(),
        next_name: NumCell::new(1),
        globals,
        formats: format::formats(),
        output_ids: Default::default(),
        root: Rc::new(DisplayNode::new(node_ids.next())),
        node_ids,
        backend_events: AsyncQueue::new(),
        output_handlers: Default::default(),
        seat_ids: Default::default(),
        seat_handlers: Default::default(),
    });
    let _global_event_handler = engine.spawn(tasks::handle_backend_events(state.clone()));
    Acceptor::install(&state)?;
    let _backend = XorgBackend::new(&state)?;
    el.run()?;
    Ok(())
}
