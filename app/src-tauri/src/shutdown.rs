use crate::{
    clusters::commands::ClusterSession, engine::EngineState, local_vm::LocalVm, send::SendOwner,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Condvar, Mutex,
};
use std::time::Duration;

#[derive(Default)]
struct Completion {
    result: Option<bool>,
    native_exit: bool,
}

/// One terminal cleanup worker, shared by native exit requests and its completion.
#[derive(Default)]
pub struct TerminalShutdown {
    closing: AtomicBool,
    completion: Mutex<Completion>,
    finished: Condvar,
}

impl TerminalShutdown {
    pub fn completed(&self) -> bool {
        self.result().is_some()
    }

    pub fn closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    pub fn result(&self) -> Option<bool> {
        self.completion.lock().unwrap().result
    }

    /// Claim final exit before requesting cleanup. A completed Mac request returns Now.
    pub fn native_termination(&self) -> Option<bool> {
        let mut completion = self.completion.lock().unwrap();
        completion.native_exit = true;
        completion.result
    }

    /// Queue ordinary exit under the same lock as native ownership/result publication.
    /// A native callback racing an already queued exit sees completed, never defers.
    pub fn exit_unless_native(&self, exit: impl FnOnce(bool)) {
        let completion = self.completion.lock().unwrap();
        if !completion.native_exit {
            if let Some(clean) = completion.result {
                exit(clean);
            }
        }
    }

    /// The OS thread may block, but cleanup must not depend on that thread's event loop.
    pub fn wait_timeout(&self, timeout: Duration) -> Option<bool> {
        self.finished
            .wait_timeout_while(self.completion.lock().unwrap(), timeout, |c| {
                c.result.is_none()
            })
            .unwrap()
            .0
            .result
    }

    /// Run on the native event thread before scheduling the winning caller's worker.
    pub fn request(&self, vm: &LocalVm, engine: &EngineState) -> bool {
        if self.closing.swap(true, Ordering::SeqCst) {
            return false;
        }
        vm.request_close(); // Bar late Ready before scheduling async work.
        engine.shutdown();
        true
    }

    /// Only the request winner runs cleanup. The caller retains the worker until exit.
    pub async fn finish(&self, clusters: &ClusterSession, send: &SendOwner, vm: &LocalVm) -> bool {
        clusters.shutdown().await; // Reap credential children without acquiring discovery's session lock.
        send.shutdown().await; // Stop observation and close the forward; remote Tasks continue.
        let clean = vm.shutdown().await.is_ok();
        if !clean {
            eprintln!("Local VM startup or shutdown did not finish successfully; cleanup was not reported as clean.");
        }
        self.completion.lock().unwrap().result = Some(clean);
        self.finished.notify_all();
        clean
    }
}
