// Exercise the native storage owner on Linux as well as native desktop CI.
extern crate self as bridge_engine;
#[path = "../../../app/src-tauri/src/agent_folder.rs"]
pub mod agent_folder;
#[path = "../../../app/src-tauri/src/runtime/snapshot.rs"]
pub mod snapshot;
pub mod runtime {
    pub use crate::snapshot;
}
include!("../../../app/src-tauri/tests/agent_folder.rs");

#[test]
fn created_stage_requires_its_owner_and_both_committed_files() {
    let stage = tempfile::tempdir().unwrap();
    let dir =
        cap_std::fs::Dir::open_ambient_dir(stage.path(), cap_std::ambient_authority()).unwrap();
    let id = agent_folder::DocumentId(uuid::Uuid::new_v4());
    let owner = AgentFolder::created(None, "helper", id).unwrap();
    owner.seed_created(&dir, None).unwrap();
    owner.verify_created(&dir).unwrap();
    for name in ["helper.agent.md", "helper.agent.test.md"] {
        assert_eq!(
            committed(stage.path(), name),
            fs::read(stage.path().join(name)).unwrap()
        );
    }
    let other = AgentFolder::created(
        None,
        "helper",
        agent_folder::DocumentId(uuid::Uuid::new_v4()),
    )
    .unwrap();
    assert!(other.verify_created(&dir).is_err());
    fs::remove_file(stage.path().join("helper.agent.test.md")).unwrap();
    assert!(owner.verify_created(&dir).is_err());
}
