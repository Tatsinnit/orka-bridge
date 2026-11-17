use bridge_engine::{
    agent_folder::DocumentId,
    agents::Agents,
    engine::run::{ParsedAgent, ParsedPair},
    overview::saved_files,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
#[path = "support/created_agent.rs"]
mod created_agent;

fn owner(home: &Path) -> Agents {
    Agents::new(Some(home.into()), Some(home.join("fixture-data")))
}
fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.insert(path.strip_prefix(root).unwrap().into(), Vec::new());
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(path, path, &mut files);
    files
}
fn assert_blank_draft(agents: &Agents, home: &Path, id: DocumentId) {
    let pair = agents.get(id).unwrap().inspect().unwrap().unwrap();
    assert!(ParsedAgent::parse(pair.agent.as_bytes()).is_err());
    assert!(ParsedPair::parse(pair.agent.as_bytes(), pair.tests.as_bytes()).is_err());
    agents.select(id).unwrap();
    let before = tree(home);
    let inventory = saved_files(agents, false);
    assert_eq!(inventory.state, "complete");
    assert_eq!(inventory.files.len(), 1);
    let file = &inventory.files[0];
    assert_eq!(file.document_id, id);
    assert_eq!(file.name, pair.name);
    assert_eq!(file.state, "readable");
    assert_eq!(file.source.as_ref(), Some(&pair.source));
    assert_eq!(serde_json::to_value(file).unwrap()["draft"], true);
    assert!(file.summary.is_none());
    assert_eq!(
        file.reason,
        Some("Add instructions and a test to get started.")
    );
    assert_eq!(agents.selected().unwrap(), id);
    assert_eq!(before, tree(home));
}

#[test]
fn untouched_private_blanks_are_drafts_without_mutating_the_pair_or_selection() {
    for (model, crlf) in [
        (None, false),
        (Some("provider/model"), false),
        (None, true),
        (Some("quote\"and\\slash"), true),
    ] {
        let home = tempfile::tempdir().unwrap();
        let agents = owner(home.path());
        let id = agents.create("helper", model).unwrap();
        let root = home.path().join("Orka/agents/helper");
        let pair = agents.get(id).unwrap().inspect().unwrap().unwrap();
        assert!(pair.agent.contains(&format!(
            "model: {}\n",
            serde_json::to_string(model.unwrap_or("gpt-5.6-terra")).unwrap()
        )));
        if crlf {
            fs::write(
                root.join("helper.agent.md"),
                pair.agent.replace('\n', "\r\n"),
            )
            .unwrap();
            fs::write(
                root.join("helper.agent.test.md"),
                pair.tests.replace('\n', "\r\n"),
            )
            .unwrap();
        }
        // Classification must come from the saved pair, not creation-session state.
        assert_blank_draft(&owner(home.path()), home.path(), id);
    }
}

#[test]
fn untouched_repo_blanks_are_drafts_including_the_reserved_sample_name() {
    for (model, crlf) in [(None, false), (Some("claude-sonnet-5-5"), true)] {
        let home = tempfile::tempdir().unwrap();
        let checkout = home.path().join("checkout");
        git2::Repository::init(&checkout).unwrap();
        let agents = owner(home.path());
        let repo = agents.bind_repo(&checkout).unwrap();
        let id = agents
            .create_with_repo("web-reader", model, Some(repo.id))
            .unwrap();
        let pair = agents.get(id).unwrap().inspect().unwrap().unwrap();
        if crlf {
            for (name, text) in [
                ("web-reader.agent.md", pair.agent),
                ("web-reader.agent.test.md", pair.tests),
            ] {
                fs::write(
                    checkout.join("web-reader").join(name),
                    text.replace('\n', "\r\n"),
                )
                .unwrap();
            }
        }
        assert_blank_draft(&owner(home.path()), home.path(), id);
    }
}

#[test]
fn malformed_unsupported_or_edited_blank_pairs_are_not_drafts() {
    let home = tempfile::tempdir().unwrap();
    let agents = owner(home.path());
    let id = agents.create("helper", None).unwrap();
    let pair = agents.get(id).unwrap().inspect().unwrap().unwrap();
    let root = home.path().join("Orka/agents/helper");
    let changed_agents = [
        pair.agent.replace("runtime: orka", "runtime: unsupported"),
        pair.agent.replace("schemaVersion: 1", "schemaVersion: 2"),
        pair.agent
            .replace("name: \"helper\"", "name: \"different\""),
        pair.agent
            .replace("model: \"gpt-5.6-terra\"", "model: gpt-5.6-terra"),
        pair.agent
            .replace("model: \"gpt-5.6-terra\"", "model: \"\""),
        pair.agent
            .replace("model: \"gpt-5.6-terra\"", "model: \"AuTo\""),
        pair.agent
            .replace("model: \"gpt-5.6-terra\"", "model: \"has space\""),
        pair.agent.replace("gpt-5.6-terra", &"m".repeat(257)),
        pair.agent.replace("gpt-5.6-terra", "é"),
        pair.agent.replace("kmx:", "unknown: true\nkmx:"),
        pair.agent.replacen("---", "--", 1),
        format!("{}Use the tools to answer questions.\n", pair.agent),
        format!("{}<!-- edited -->\n", pair.agent),
        " \n".into(),
    ];
    let changed_tests = [
        pair.tests.replace("input: \"\"", "input: a question"),
        pair.tests.replace("assertions: []", "assertions: ["),
        pair.tests.replace("# helper tests", "# different tests"),
        pair.tests.replace("schemaVersion: 1", "schemaVersion: 2"),
        format!("{}<!-- edited -->\n", pair.tests),
        String::new(),
        pair.tests
            .replace("input: \"\"", "input: a question")
            .replace(
                "assertions: []",
                "assertions: [{id: answer, type: contains, value: yes}]",
            ),
    ];
    for (agent, tests) in changed_agents
        .iter()
        .map(|agent| (agent, &pair.tests))
        .chain(changed_tests.iter().map(|tests| (&pair.agent, tests)))
    {
        fs::write(root.join("helper.agent.md"), agent).unwrap();
        fs::write(root.join("helper.agent.test.md"), tests).unwrap();
        assert!(ParsedPair::parse(agent.as_bytes(), tests.as_bytes()).is_err());
        let before = tree(home.path());
        let inventory = saved_files(&agents, false);
        let file = &inventory.files[0];
        assert_eq!(file.state, "readable");
        assert!(file.source.is_some());
        assert_eq!(serde_json::to_value(file).unwrap()["draft"], false);
        assert!(file.summary.is_none());
        assert_eq!(
            file.reason,
            Some("The saved declarations are unsupported or invalid.")
        );
        assert_eq!(before, tree(home.path()));
    }
}

#[test]
fn the_builtin_sample_is_not_reclassified_as_a_created_blank() {
    let home = tempfile::tempdir().unwrap();
    let agents = owner(home.path());
    let id = agents.create("helper", None).unwrap();
    let blank = agents.get(id).unwrap().inspect().unwrap().unwrap();
    agents
        .sample()
        .save(blank.agent.replace("helper", "web-reader"))
        .unwrap();
    fs::write(
        home.path()
            .join("Orka/agents/web-reader/web-reader.agent.test.md"),
        blank.tests.replace("helper", "web-reader"),
    )
    .unwrap();
    let inventory = saved_files(&agents, false);
    let sample = &inventory.files[0];
    assert_eq!(sample.document_id, DocumentId::SAMPLE);
    assert_eq!(serde_json::to_value(sample).unwrap()["draft"], false);
    assert!(sample.summary.is_none());
    assert_eq!(
        sample.reason,
        Some("The saved declarations are unsupported or invalid.")
    );
}

#[test]
fn registry_observation_is_non_mutating_and_keeps_document_identity() {
    let home = tempfile::tempdir().unwrap();
    let agents = owner(home.path());
    let pair = agents.sample().open().unwrap();
    let a = created_agent::created(home.path(), "alpha", &pair.agent, &pair.tests);
    let b = created_agent::created(home.path(), "beta", &pair.agent, &pair.tests);
    let before = tree(home.path());
    let files = saved_files(&agents, true);
    assert!(files.run_active);
    assert_eq!(files.state, "complete");
    assert_eq!(files.files.len(), 3);
    assert!(files
        .files
        .iter()
        .all(|file| serde_json::to_value(file).unwrap()["draft"] == false));
    assert_eq!(files.files[1].document_id, a.document_id());
    assert_eq!(files.files[2].document_id, b.document_id());
    assert_ne!(a.document_id(), b.document_id());
    assert_eq!(files.files[1].source, files.files[2].source);
    assert_eq!(
        files.files[0].summary.as_ref().unwrap().model,
        "gpt-5.6-terra"
    );
    assert_eq!(before, tree(home.path()));
}
#[test]
fn projects_each_pair_before_reading_the_next_folder() {
    let home = tempfile::tempdir().unwrap();
    let agents = owner(home.path());
    let pair = agents.sample().open().unwrap();
    created_agent::created(home.path(), "alpha", &pair.agent, &pair.tests);
    let beta = created_agent::created(home.path(), "beta", &pair.agent, &pair.tests);
    let changed = format!("{}\nEdited before beta is inspected", pair.agent);
    let expected =
        bridge_runtime_protocol::SnapshotId::of(changed.as_bytes(), pair.tests.as_bytes());
    let observed = agents
        .inspect_files(|file| {
            if file.document.name == "alpha" {
                fs::write(home.path().join("Orka/agents/beta/beta.agent.md"), &changed).unwrap();
            }
            // Retain only the projected identity, never source text from previous pairs.
            Some((
                file.document.document_id,
                file.pair.unwrap().unwrap().source,
            ))
        })
        .unwrap();
    assert_eq!(
        observed
            .iter()
            .find(|(id, _)| *id == beta.document_id())
            .unwrap()
            .1,
        expected
    );
}
#[test]
fn dirty_reopen_is_not_committed_and_unsupported_source_is_unavailable() {
    let home = tempfile::tempdir().unwrap();
    let agents = owner(home.path());
    let pair = agents.sample().open().unwrap();
    let path = home
        .path()
        .join("Orka/agents/web-reader/web-reader.agent.md");
    fs::write(&path, format!("{}\nUncommitted prompt", pair.agent)).unwrap();
    let before = tree(home.path());
    let observed = saved_files(&agents, false);
    assert_ne!(observed.files[0].source.as_ref().unwrap(), &pair.source);
    assert!(observed.files[0].summary.is_some());
    assert_eq!(before, tree(home.path()));
    fs::write(
        &path,
        pair.agent.replace("runtime: orka", "runtime: unsupported"),
    )
    .unwrap();
    let before = tree(home.path());
    let observed = saved_files(&agents, false);
    assert_eq!(observed.files[0].state, "readable");
    assert!(observed.files[0].source.is_some());
    assert!(observed.files[0].summary.is_none());
    assert!(observed.files[0].reason.is_some());
    assert_eq!(before, tree(home.path()));
}
#[test]
fn missing_registered_pair_and_bad_registry_are_not_empty_successes() {
    let home = tempfile::tempdir().unwrap();
    let agents = owner(home.path());
    let pair = agents.sample().open().unwrap();
    created_agent::created(home.path(), "alpha", &pair.agent, &pair.tests);
    fs::remove_dir_all(home.path().join("Orka/agents/alpha")).unwrap();
    let files = saved_files(&agents, false);
    assert_eq!(files.files.len(), 2);
    assert_eq!(files.files[1].state, "unreadable");
    assert_eq!(
        serde_json::to_value(&files.files[1]).unwrap()["draft"],
        false
    );
    fs::write(
        home.path().join("fixture-data/agents-v1/registry.json"),
        b"not json",
    )
    .unwrap();
    assert_eq!(saved_files(&agents, false).state, "unavailable");
}
#[cfg(unix)]
#[test]
fn ownership_and_nofollow_are_preserved() {
    use std::os::unix::fs::symlink;
    for leaf in [
        ".bridge-agent-folder",
        "web-reader.agent.md",
        "web-reader.agent.test.md",
        ".git",
    ] {
        let home = tempfile::tempdir().unwrap();
        let agents = owner(home.path());
        agents.sample().open().unwrap();
        let root = home.path().join("Orka/agents/web-reader");
        let original = root.join(leaf);
        let outside = home.path().join("outside");
        fs::rename(&original, &outside).unwrap();
        symlink(&outside, original).unwrap();
        assert_eq!(
            saved_files(&agents, false).files[0].state,
            "unreadable",
            "{leaf}"
        );
    }
    let home = tempfile::tempdir().unwrap();
    let agents = owner(home.path());
    agents.sample().open().unwrap();
    fs::write(
        home.path()
            .join("Orka/agents/web-reader/.bridge-agent-folder"),
        b"other owner",
    )
    .unwrap();
    assert_eq!(saved_files(&agents, false).files[0].state, "unreadable");
}
#[test]
fn projection_excludes_paths_source_bytes_and_diagnostics() {
    let home = tempfile::tempdir().unwrap();
    let agents = owner(home.path());
    let pair = agents.sample().open().unwrap();
    agents
        .sample()
        .save(format!("{}\nPRIVATE PROMPT BODY", pair.agent))
        .unwrap();
    let value = serde_json::to_string(&saved_files(&agents, false)).unwrap();
    for (index, forbidden) in [
        "PRIVATE PROMPT BODY",
        "path",
        home.path().to_str().unwrap(),
        "schemaVersion",
        "agent-test",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            !value.contains(forbidden),
            "Forbidden Overview field {index}"
        );
    }
}
#[test]
fn missing_is_not_seeded() {
    let home = tempfile::tempdir().unwrap();
    let files = saved_files(&owner(home.path()), false);
    assert!(files.files.is_empty());
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}
