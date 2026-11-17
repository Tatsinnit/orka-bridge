use bridge_engine::{
    agent_folder::AgentFolder,
    example_run::ExampleRun,
    local_vm::{LocalFactory, LocalProgress, LocalSession, LocalVm, LocalVmStatus, StartFuture},
    runtime::types::RuntimeError,
};
use std::sync::Arc;
use tokio::sync::watch;
struct Factory;
struct Session;
impl LocalSession for Session {
    fn execute(
        &mut self,
        _: bridge_engine::engine::types::EngineRequest,
    ) -> bridge_engine::engine::TransportFuture<'_> {
        Box::pin(async { panic!("admission fixture never executes engine work") })
    }
    fn stop(&mut self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
impl LocalFactory for Factory {
    fn launch(
        &self,
        _: watch::Receiver<bool>,
        _: Arc<dyn Fn(LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        Box::pin(async { Ok(Box::new(Session) as Box<dyn LocalSession>) })
    }
}
async fn ready(vm: &Arc<LocalVm>) {
    vm.start();
    for _ in 0..1000 {
        if vm.status() == LocalVmStatus::Ready {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("fixture launch did not finish: {:?}", vm.status());
}
#[tokio::test]
async fn run_status_reflects_chat_admission_even_with_no_run_in_progress() {
    let vm = LocalVm::new(Arc::new(Factory));
    ready(&vm).await;
    let home = tempfile::tempdir().unwrap();
    let run = ExampleRun::new(
        tokio::runtime::Handle::current(),
        true,
        Arc::new(AgentFolder::new(Some(home.path().into()))),
    );
    assert!(run.status(&vm).can_run);
    let lease = vm.try_authoring().unwrap();
    assert!(!run.status(&vm).can_run);
    assert!(!run.start(vm.clone()).can_run);
    assert!(!run.is_active());
    drop(lease);
    assert!(run.status(&vm).can_run);
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn new_workspace_admission_is_not_held_by_an_unresolved_old_boot() {
    let vm = LocalVm::new(Arc::new(Factory));
    ready(&vm).await;
    let old = vm.try_authoring().unwrap();
    assert!(vm.try_authoring().is_none());
    vm.cancel().await.unwrap();
    ready(&vm).await;
    let current = vm.try_authoring().expect("new boot must recover admission");
    assert!(
        !vm.authoring_current(&old),
        "old work cannot use the new boot"
    );
    assert!(vm.authoring_current(&current));
    drop(old);
    assert!(
        vm.try_authoring().is_none(),
        "old lease cannot release current boot's lease"
    );
    drop(current);
    assert!(vm.try_authoring().is_some());
    vm.shutdown().await.unwrap();
}
