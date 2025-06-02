//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
use crate::task::Stride;
///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// A simple FIFO scheduler.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Take a process out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.ready_queue.pop_front()
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}

/// get task with the smallest stride
pub fn get_task_with_smallest_stride() -> Option<Arc<TaskControlBlock>> {
    let chosen_index;
    {
        let task_manager = TASK_MANAGER.exclusive_access();
        let mut min_stride = Stride::new(isize::MAX);
        let mut min_stride_index = 0;
        chosen_index = {
            for (index, task) in task_manager.ready_queue.iter().enumerate() {
                let task_inner = task.inner_exclusive_access();
                if task_inner.stride < min_stride {
                    min_stride.0 = task_inner.stride.0;
                    min_stride_index = index;
                }
            }

            min_stride_index
        }
    }

    {
        let task_manager = &mut TASK_MANAGER.exclusive_access();
        task_manager.ready_queue.remove(chosen_index)
    }
}

#[allow(dead_code)]
/// Print all tasks in the ready queue
pub fn print_tasks() {
    let task_manager = TASK_MANAGER.exclusive_access();
    for task in &task_manager.ready_queue {
        let task_inner: core::cell::RefMut<'_, crate::task::task::TaskControlBlockInner> = task.inner_exclusive_access();
        println!(
            "Task PID: {}, Stride: {}",
            task.pid.0, task_inner.stride.0
        );
    }
}