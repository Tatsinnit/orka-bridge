use bridge_engine::{
    agent_folder::{AgentFolder, DocumentId},
    agents::Agents,
};
use std::{fs, path::Path, sync::Arc};

// Native-owned publication fixture, not an app creation or relabelling path.
pub fn created(home: &Path, name: &str, agent: &str, tests: &str) -> Arc<AgentFolder> {
    let id: DocumentId =
        serde_json::from_value(serde_json::json!(bridge_runtime_protocol::Uuid::new_v4())).unwrap();
    let root = home.join("Orka/agents").join(name);
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::create_dir(root.join(".git/refs")).unwrap();
    fs::create_dir(root.join(".git/info")).unwrap();
    fs::write(root.join(".git/info/exclude"), b"/.bridge-agent-folder\n").unwrap();
    fs::write(
        root.join(".git/config"),
        b"[core]\nrepositoryformatversion = 0\nbare = false\n",
    )
    .unwrap();
    fs::write(root.join(".git/HEAD"), b"").unwrap();
    fs::write(
        root.join(".bridge-agent-folder"),
        format!(
            "Bridge agent folder v1\n{}\n",
            serde_json::to_value(id).unwrap().as_str().unwrap()
        ),
    )
    .unwrap();
    fs::write(root.join(format!("{name}.agent.md")), agent).unwrap();
    fs::write(root.join(format!("{name}.agent.test.md")), tests).unwrap();
    let agents = Agents::new(Some(home.into()), Some(home.join("fixture-data")));
    assert_eq!(agents.register_created(name).unwrap(), id);
    agents.get(id).unwrap()
}
