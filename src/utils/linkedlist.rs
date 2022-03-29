use crate::utils::numcell::NumCell;
use crate::utils::ptr_ext::PtrExt;
use std::cell::Cell;
use std::fmt::{Debug, Formatter};
use std::mem;
use std::ops::Deref;
use std::ptr::NonNull;

pub struct LinkedList<T> {
    root: LinkedNode<T>,
}

impl<T: Debug> Debug for LinkedList<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T> Default for LinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LinkedList<T> {
    pub fn new() -> Self {
        let node = Box::into_raw(Box::new(NodeData {
            rc: NumCell::new(1),
            prev: Cell::new(NonNull::dangling()),
            next: Cell::new(NonNull::dangling()),
            data: None,
        }));
        unsafe {
            node.deref().prev.set(NonNull::new_unchecked(node));
            node.deref().next.set(NonNull::new_unchecked(node));
            Self {
                root: LinkedNode {
                    data: NonNull::new_unchecked(node),
                },
            }
        }
    }

    fn endpoint(&self, ep: NonNull<NodeData<T>>) -> Option<NodeRef<T>> {
        unsafe {
            if ep != self.root.data {
                ep.as_ref().rc.fetch_add(1);
                Some(NodeRef { data: ep })
            } else {
                None
            }
        }
    }

    pub fn last(&self) -> Option<NodeRef<T>> {
        unsafe { self.endpoint(self.root.data.as_ref().prev.get()) }
    }

    #[allow(dead_code)]
    pub fn first(&self) -> Option<NodeRef<T>> {
        unsafe { self.endpoint(self.root.data.as_ref().next.get()) }
    }

    pub fn add_last(&self, t: T) -> LinkedNode<T> {
        self.root.prepend(t)
    }

    pub fn add_first(&self, t: T) -> LinkedNode<T> {
        self.root.append(t)
    }

    pub fn iter(&self) -> LinkedListIter<T> {
        unsafe {
            let root = self.root.data.as_ref();
            root.rc.fetch_add(1);
            root.next.get().as_ref().rc.fetch_add(1);
            LinkedListIter {
                root: self.root.data,
                next: root.next.get(),
            }
        }
    }

    pub fn rev_iter(&self) -> RevLinkedListIter<T> {
        unsafe {
            let root = self.root.data.as_ref();
            root.rc.fetch_add(1);
            root.prev.get().as_ref().rc.fetch_add(1);
            RevLinkedListIter {
                root: self.root.data,
                next: root.prev.get(),
            }
        }
    }
}

pub struct LinkedListIter<T> {
    root: NonNull<NodeData<T>>,
    next: NonNull<NodeData<T>>,
}

impl<T> Iterator for LinkedListIter<T> {
    type Item = NodeRef<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.root == self.next {
            return None;
        }
        unsafe {
            let old_next = self.next;
            self.next = old_next.as_ref().next.get();
            self.next.as_ref().rc.fetch_add(1);
            Some(NodeRef { data: old_next })
        }
    }
}

impl<T> Drop for LinkedListIter<T> {
    fn drop(&mut self) {
        unsafe {
            dec_ref_count(self.root, 1);
            dec_ref_count(self.next, 1);
        }
    }
}

pub struct RevLinkedListIter<T> {
    root: NonNull<NodeData<T>>,
    next: NonNull<NodeData<T>>,
}

impl<T> Iterator for RevLinkedListIter<T> {
    type Item = NodeRef<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.root == self.next {
            return None;
        }
        unsafe {
            let old_next = self.next;
            self.next = old_next.as_ref().prev.get();
            self.next.as_ref().rc.fetch_add(1);
            Some(NodeRef { data: old_next })
        }
    }
}

impl<T> Drop for RevLinkedListIter<T> {
    fn drop(&mut self) {
        unsafe {
            dec_ref_count(self.root, 1);
            dec_ref_count(self.next, 1);
        }
    }
}

#[repr(transparent)]
pub struct LinkedNode<T> {
    data: NonNull<NodeData<T>>,
}

impl<T: Debug> Debug for LinkedNode<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        unsafe { self.data.as_ref().data.as_ref().unwrap_unchecked().fmt(f) }
    }
}

impl<T> Deref for LinkedNode<T> {
    type Target = NodeRef<T>;

    fn deref(&self) -> &Self::Target {
        unsafe { mem::transmute(self) }
    }
}

#[repr(transparent)]
pub struct NodeRef<T> {
    data: NonNull<NodeData<T>>,
}

impl<T: Debug> Debug for NodeRef<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        unsafe { self.data.as_ref().data.as_ref().unwrap_unchecked().fmt(f) }
    }
}

impl<T> Deref for NodeRef<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.data.as_ref().data.as_ref().unwrap_unchecked() }
    }
}

impl<T> Drop for NodeRef<T> {
    fn drop(&mut self) {
        unsafe {
            dec_ref_count(self.data, 1);
        }
    }
}

impl<T> Clone for NodeRef<T> {
    fn clone(&self) -> Self {
        unsafe {
            self.data.as_ref().rc.fetch_add(1);
            Self { data: self.data }
        }
    }
}

impl<T> NodeRef<T> {
    pub fn prepend(&self, t: T) -> LinkedNode<T> {
        unsafe { prepend(self.data, t) }
    }

    pub fn append(&self, t: T) -> LinkedNode<T> {
        unsafe { append(self.data, t) }
    }

    fn peer<F>(&self, peer: F) -> Option<NodeRef<T>>
    where
        F: FnOnce(&NodeData<T>) -> &Cell<NonNull<NodeData<T>>>,
    {
        unsafe {
            let data = self.data.as_ref();
            let other = peer(data).get();
            if other.as_ref().data.is_some() {
                other.as_ref().rc.fetch_add(1);
                Some(NodeRef { data: other })
            } else {
                None
            }
        }
    }

    pub fn prev(&self) -> Option<NodeRef<T>> {
        self.peer(|d| &d.prev)
    }

    pub fn next(&self) -> Option<NodeRef<T>> {
        self.peer(|d| &d.next)
    }
}

struct NodeData<T> {
    rc: NumCell<usize>,
    prev: Cell<NonNull<NodeData<T>>>,
    next: Cell<NonNull<NodeData<T>>>,
    data: Option<T>,
}

unsafe fn dec_ref_count<T>(slf: NonNull<NodeData<T>>, n: usize) {
    if slf.as_ref().rc.fetch_sub(n) == n {
        drop(Box::from_raw(slf.as_ptr()));
    }
}

impl<T> Drop for LinkedNode<T> {
    fn drop(&mut self) {
        unsafe {
            {
                let data = self.data.as_ref();
                data.prev.get().as_ref().next.set(data.next.get());
                data.next.get().as_ref().prev.set(data.prev.get());
                data.prev.set(self.data);
                data.next.set(self.data);
            }
            dec_ref_count(self.data, 1);
        }
    }
}

impl<T> LinkedNode<T> {
    pub fn prepend(&self, t: T) -> LinkedNode<T> {
        unsafe { prepend(self.data, t) }
    }

    pub fn append(&self, t: T) -> LinkedNode<T> {
        unsafe { append(self.data, t) }
    }

    pub fn to_ref(&self) -> NodeRef<T> {
        unsafe {
            self.data.as_ref().rc.fetch_add(1);
            NodeRef { data: self.data }
        }
    }
}

unsafe fn prepend<T>(data: NonNull<NodeData<T>>, t: T) -> LinkedNode<T> {
    let dref = data.as_ref();
    let node = NonNull::new_unchecked(Box::into_raw(Box::new(NodeData {
        rc: NumCell::new(1),
        prev: Cell::new(dref.prev.get()),
        next: Cell::new(data),
        data: Some(t),
    })));
    dref.prev.get().as_ref().next.set(node);
    dref.prev.set(node);
    LinkedNode { data: node }
}

unsafe fn append<T>(data: NonNull<NodeData<T>>, t: T) -> LinkedNode<T> {
    let dref = data.as_ref();
    let node = NonNull::new_unchecked(Box::into_raw(Box::new(NodeData {
        rc: NumCell::new(1),
        prev: Cell::new(data),
        next: Cell::new(dref.next.get()),
        data: Some(t),
    })));
    dref.next.get().as_ref().prev.set(node);
    dref.next.set(node);
    LinkedNode { data: node }
}
