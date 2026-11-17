use bridge_engine::agent_folder::AgentFolder;
use bridge_runtime_protocol::{FileSlot, MAX_FILE};
use std::{fs, path::Path, sync::Arc};

const AGENT: &str = "web-reader.agent.md";
const TESTS: &str = "web-reader.agent.test.md";
fn root(home: &Path) -> std::path::PathBuf {
    home.join("Orka").join("agents").join("web-reader")
}
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
fn committed(root: &Path, name: &str) -> Vec<u8> {
    let repo = git2::Repository::open(root).unwrap();
    let tree = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    let id = tree.get_name(name).unwrap().id();
    let blob = repo.find_blob(id).unwrap();
    blob.content().to_vec()
}
#[test]
fn prepare_example_sets_only_the_closed_model_line_and_commits_it() {
    for model in ["gpt-5.6-terra", "gpt-4.1", "claude-sonnet-5-5"] {
        let home = tempfile::tempdir().unwrap();
        let folder = AgentFolder::new(Some(home.path().into()));
        let original = include_str!("../example-agent/web-reader.agent.md");
        let pair = folder.prepare_example_model(model).unwrap();
        assert_eq!(
            pair.agent,
            original.replace("model: gpt-5.6-terra\n", &format!("model: {model}\n"))
        );
        assert!(pair.agent.contains(&format!("model: {model}\n")));
        assert_eq!(
            pair.tests,
            include_str!("../example-agent/web-reader.agent.test.md")
        );
        assert_eq!(committed(&root(home.path()), AGENT), pair.agent.as_bytes());
        assert_eq!(folder.capture().unwrap().id(), pair.source);
    }
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    assert!(folder.prepare_example_model("arbitrary-model").is_err());
    assert!(!root(home.path()).exists());
}
#[test]
fn preparation_preserves_edited_pairs_and_an_already_prepared_model() {
    for leaf in [AGENT, TESTS] {
        let home = tempfile::tempdir().unwrap();
        let folder = AgentFolder::new(Some(home.path().into()));
        let before = folder.open().unwrap();
        let file = root(home.path()).join(leaf);
        fs::write(
            &file,
            format!("{}\nEdited.\n", fs::read_to_string(&file).unwrap()),
        )
        .unwrap();
        let edited = folder.open().unwrap();
        let prepared = folder.prepare_example_model("gpt-4.1").unwrap();
        assert_eq!(prepared.agent, edited.agent);
        assert_eq!(prepared.tests, edited.tests);
        assert_ne!(prepared.source, before.source);
    }
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    let prepared = folder.prepare_example_model("gpt-4.1").unwrap();
    let switched = folder.prepare_example_model("claude-sonnet-5-5").unwrap();
    assert_eq!(switched.source, prepared.source);
    assert_eq!(switched.agent, prepared.agent);
}
#[test]
fn initializes_without_vm_and_reopens_exact_drafts_with_committed_bytes() {
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    let pair = folder.open().unwrap();
    assert_eq!(pair.path.as_deref(), root(home.path()).join(AGENT).to_str());
    assert_eq!(folder.save(pair.agent.clone()).unwrap().path, pair.path);
    assert!(pair.agent.contains("web_fetch"));
    assert!(pair.tests.contains("toolCalled"));
    let (path, doc) = folder.source().unwrap();
    assert_eq!(path, root(home.path()));
    assert_eq!(doc.agent, Path::new(AGENT));
    let draft = "\r\nnot schema 🦀\n";
    assert_eq!(folder.save(draft.into()).unwrap().agent, draft);
    assert_eq!(committed(&path, AGENT), draft.as_bytes());
    assert_eq!(committed(&path, TESTS), pair.tests.as_bytes());
    let reopened = AgentFolder::new(Some(home.path().into()));
    assert_eq!(reopened.open().unwrap().agent, draft);
    assert_eq!(
        reopened.capture().unwrap().file(FileSlot::Agent),
        draft.as_bytes()
    );
    assert_eq!(reopened.save(String::new()).unwrap().agent, "");
    assert_eq!(committed(&path, AGENT), b"");
}
#[test]
fn packed_history_reopens_saves_and_captures_exact_bytes() {
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    let original = folder.open().unwrap();
    let path = root(home.path());
    let head = fs::read_to_string(path.join(".git/HEAD")).unwrap();
    git(&path, &["gc", "--prune=now"]);
    // The packed reader must not discover the creator's external object stores.
    fs::write(
        path.join(".git/objects/info/alternates"),
        b"/not-an-object-store\n",
    )
    .unwrap();
    // Side files are ordinary Git metadata, not inputs to the packed HEAD reader.
    fs::write(path.join(".git/objects/pack/extra.keep"), b"keep").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "/not-a-pack-input",
        path.join(".git/objects/pack/extra.bitmap"),
    )
    .unwrap();
    let name = head.trim();
    assert!(!path
        .join(".git/objects")
        .join(&name[..2])
        .join(&name[2..])
        .exists());
    let reopened = AgentFolder::new(Some(home.path().into()));
    assert_eq!(reopened.open().unwrap().agent, original.agent);
    let draft = "edited after packing\r\n🦀";
    assert_eq!(reopened.save(draft.into()).unwrap().agent, draft);
    assert_eq!(committed(&path, AGENT), draft.as_bytes());
    assert_eq!(committed(&path, TESTS), original.tests.as_bytes());
    let repo = git2::Repository::open(&path).unwrap();
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.parent_id(0).unwrap().to_string(), name);
    let snapshot = reopened.capture().unwrap();
    assert_eq!(snapshot.file(FileSlot::Agent), draft.as_bytes());
    assert_eq!(snapshot.file(FileSlot::Tests), original.tests.as_bytes());
    drop(commit);
    drop(repo);
    fs::remove_file(path.join(".git/objects/info/alternates")).unwrap();
    git(&path, &["gc", "--prune=now"]);
    assert_eq!(
        reopened.capture().unwrap().file(FileSlot::Agent),
        draft.as_bytes()
    );
    git(&path, &["fsck", "--full"]);
    assert_eq!(git(&path, &["status", "--porcelain"]), b"");
}
#[cfg(unix)]
#[test]
fn packed_inputs_keep_no_follow_and_single_link_boundaries() {
    use std::os::unix::fs::symlink;
    for (extension, hardlink) in [
        ("directory", false),
        ("idx", false),
        ("pack", false),
        ("idx", true),
        ("pack", true),
    ] {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let folder = AgentFolder::new(Some(home.path().into()));
        let original = folder.open().unwrap();
        let path = root(home.path());
        git(&path, &["gc", "--prune=now"]);
        let head = fs::read(path.join(".git/HEAD")).unwrap();
        let packs = path.join(".git/objects/pack");
        if extension == "directory" {
            fs::rename(&packs, outside.path().join("pack")).unwrap();
            symlink(outside.path().join("pack"), &packs).unwrap();
        } else {
            let input = fs::read_dir(&packs)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|name| name.extension().is_some_and(|ext| ext == extension))
                .unwrap();
            let target = outside.path().join("input");
            if hardlink {
                fs::hard_link(&input, &target).unwrap();
            } else {
                fs::rename(&input, &target).unwrap();
                symlink(&target, &input).unwrap();
            }
        }
        assert!(folder.open().is_err(), "{extension} hardlink={hardlink}");
        assert!(folder.save("must not replace".into()).is_err());
        assert!(folder.capture().is_err());
        assert_eq!(
            fs::read(path.join(AGENT)).unwrap(),
            original.agent.as_bytes()
        );
        assert_eq!(fs::read(path.join(".git/HEAD")).unwrap(), head);
    }
}
#[test]
fn corrupt_packed_or_loose_head_refuses_before_replacing_the_draft() {
    for damaged in ["idx", "pack", "loose", "directory"] {
        let home = tempfile::tempdir().unwrap();
        let folder = AgentFolder::new(Some(home.path().into()));
        let original = folder.open().unwrap();
        let path = root(home.path());
        git(&path, &["gc", "--prune=now"]);
        let head = fs::read(path.join(".git/HEAD")).unwrap();
        if damaged == "loose" {
            let name = std::str::from_utf8(&head).unwrap().trim();
            let prefix = path.join(".git/objects").join(&name[..2]);
            fs::create_dir_all(&prefix).unwrap();
            fs::write(prefix.join(&name[2..]), b"corrupt loose HEAD").unwrap();
        } else {
            let input = fs::read_dir(path.join(".git/objects/pack"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|name| {
                    name.extension().is_some_and(|ext| {
                        ext == if damaged == "directory" {
                            "pack"
                        } else {
                            damaged
                        }
                    })
                })
                .unwrap();
            // Git makes packs read-only; replace the fixture instead of truncating it.
            fs::rename(&input, input.with_extension("discarded")).unwrap();
            if damaged == "directory" {
                fs::create_dir(input).unwrap();
            } else {
                fs::write(input, b"corrupt packed HEAD").unwrap();
            }
        }
        assert!(folder.save("must not replace".into()).is_err(), "{damaged}");
        assert_eq!(
            fs::read(path.join(AGENT)).unwrap(),
            original.agent.as_bytes()
        );
        assert_eq!(fs::read(path.join(".git/HEAD")).unwrap(), head);
    }
}
#[test]
fn unrelated_directory_and_missing_home_never_overwrite_existing_data() {
    assert!(AgentFolder::new(None).open().is_err());
    let home = tempfile::tempdir().unwrap();
    let path = root(home.path());
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join(AGENT), b"my work").unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    assert!(folder.open().is_err());
    assert!(folder.save("replacement".into()).is_err());
    assert_eq!(fs::read(path.join(AGENT)).unwrap(), b"my work");
    assert!(!path.join(".git").exists());
}
#[test]
fn bounded_utf8_noop_and_explicit_identity_ignore_git_filters_and_config() {
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    let original = folder.open().unwrap();
    let path = root(home.path());
    let head = || {
        git2::Repository::open(&path)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap()
    };
    let first = head();
    folder.save(original.agent.clone()).unwrap();
    assert_eq!(head(), first);
    assert!(folder.save("x".repeat(MAX_FILE + 1)).is_err());
    assert_eq!(folder.open().unwrap().agent, original.agent);
    // These are never interpreted: blob/tree/commit operations see exact bytes.
    fs::write(
        path.join(".gitattributes"),
        "* filter=untrusted text eol=lf\n",
    )
    .unwrap();
    fs::write(path.join(".git/config"), "[core]\n bare=false\n[filter \"untrusted\"]\n clean=never-run-this-command\n[user]\n name=Not Bridge\n email=other@example.com\n").unwrap();
    let draft = "exact\r\n🦀";
    folder.save(draft.into()).unwrap();
    let repo = git2::Repository::open(&path).unwrap();
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.author().name().unwrap(), "Bridge");
    assert_eq!(commit.author().email().unwrap(), "bridge@orka.ai");
    assert_eq!(commit.parent_count(), 1);
    assert_eq!(committed(&path, AGENT), draft.as_bytes());
    assert_eq!(
        fs::read(path.join(TESTS)).unwrap(),
        original.tests.as_bytes()
    );
}
#[test]
fn dirty_reopen_commits_disk_not_bundled_or_head_and_failed_commit_retries() {
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    folder.open().unwrap();
    let path = root(home.path());
    fs::write(path.join(AGENT), "external draft").unwrap();
    assert_eq!(folder.open().unwrap().agent, "external draft");
    assert_eq!(committed(&path, AGENT), b"external draft");
    let head = fs::read(path.join(".git/HEAD")).unwrap();
    fs::remove_file(path.join(".git/index")).unwrap();
    fs::create_dir(path.join(".git/index")).unwrap();
    assert!(folder.save("retry draft".into()).is_err());
    // History failure refuses before overwriting the current disk version.
    assert_eq!(fs::read(path.join(AGENT)).unwrap(), b"external draft");
    assert_eq!(fs::read(path.join(".git/HEAD")).unwrap(), head);
    fs::remove_dir(path.join(".git/index")).unwrap();
    folder.save("retry draft".into()).unwrap();
    assert_eq!(committed(&path, AGENT), b"retry draft");
}
#[test]
fn save_preserves_an_external_edit_in_history_before_replacing_it() {
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    folder.open().unwrap();
    let path = root(home.path());
    fs::write(path.join(AGENT), b"external editor work").unwrap();
    folder.save("Bridge editor work".into()).unwrap();
    let repo = git2::Repository::open(&path).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    let previous = head.parent(0).unwrap().tree().unwrap();
    let blob = repo
        .find_blob(previous.get_name(AGENT).unwrap().id())
        .unwrap();
    assert_eq!(blob.content(), b"external editor work");
    assert_eq!(committed(&path, AGENT), b"Bridge editor work");
}
#[test]
fn concurrent_saves_are_serial_and_every_success_is_committed() {
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    folder.open().unwrap();
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let folder = folder.clone();
            std::thread::spawn(move || folder.save(format!("draft {i}")).unwrap())
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let repo = git2::Repository::open(root(home.path())).unwrap();
    let mut walk = repo.revwalk().unwrap();
    walk.push_head().unwrap();
    assert_eq!(walk.count(), 9);
    assert_eq!(
        committed(&root(home.path()), AGENT),
        folder.open().unwrap().agent.as_bytes()
    );
}
#[cfg(unix)]
#[test]
fn redirected_ancestors_source_and_git_never_write_outside() {
    use std::os::unix::fs::symlink;
    for redirected in ["Orka", "Orka/agents", "Orka/agents/web-reader"] {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = home.path().join(redirected);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(outside.path(), &link).unwrap();
        assert!(AgentFolder::new(Some(home.path().into()))
            .save("no".into())
            .is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
    for redirected in [AGENT, TESTS, ".git", ".git/objects", ".git/HEAD"] {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let folder = AgentFolder::new(Some(home.path().into()));
        folder.open().unwrap();
        let link = root(home.path()).join(redirected);
        let is_dir = link.is_dir();
        if is_dir {
            fs::remove_dir_all(&link).unwrap();
        } else {
            fs::remove_file(&link).unwrap();
        }
        let target = if is_dir {
            outside.path().to_path_buf()
        } else {
            let target = outside.path().join("work");
            fs::write(&target, b"outside").unwrap();
            target
        };
        symlink(&target, &link).unwrap();
        assert!(folder.save("no".into()).is_err(), "{redirected}");
        assert!(
            folder.prepare_example_model("gpt-4.1").is_err(),
            "{redirected}"
        );
        if !is_dir {
            assert_eq!(fs::read(target).unwrap(), b"outside");
        } else {
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        }
    }
}
#[test]
fn hardlinked_source_and_head_cannot_modify_outside_files() {
    for name in [AGENT, TESTS, ".git/HEAD"] {
        let home = tempfile::tempdir().unwrap();
        let folder = AgentFolder::new(Some(home.path().into()));
        folder.open().unwrap();
        let path = root(home.path());
        let outside = home.path().join("outside");
        let before = fs::read(path.join(name)).unwrap();
        fs::hard_link(path.join(name), &outside).unwrap();
        assert!(folder.save("no".into()).is_err());
        assert!(folder.prepare_example_model("gpt-4.1").is_err());
        assert_eq!(fs::read(outside).unwrap(), before);
    }
}

#[test]
fn valid_objects_with_another_zlib_encoding_can_be_reused() {
    use std::io::{Read, Write};
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    let pair = folder.open().unwrap();
    let path = root(home.path());
    let repo = git2::Repository::open(&path).unwrap();
    let tree = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    let id = tree.get_name(TESTS).unwrap().id().to_string();
    let object = path.join(".git/objects").join(&id[..2]).join(&id[2..]);
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&fs::read(&object).unwrap()[..])
        .read_to_end(&mut raw)
        .unwrap();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::none());
    encoder.write_all(&raw).unwrap();
    fs::write(&object, encoder.finish().unwrap()).unwrap();
    folder.save("another agent draft".into()).unwrap();
    assert_eq!(committed(&path, TESTS), pair.tests.as_bytes());
    assert_eq!(committed(&path, AGENT), b"another agent draft");
}
#[test]
fn ordinary_git_fsck_log_show_and_status_agree_with_saved_document() {
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    folder.save("exact Git bytes\\r\\n🦀".into()).unwrap();
    let path = root(home.path());
    let git = |args: &[&str]| git(&path, args);
    git(&["fsck", "--full"]);
    assert_eq!(
        git(&["log", "-1", "--format=%an <%ae>"]),
        b"Bridge <bridge@orka.ai>\n"
    );
    assert_eq!(
        git(&["show", "HEAD:web-reader.agent.md"]),
        b"exact Git bytes\\r\\n\xf0\x9f\xa6\x80"
    );
    assert_eq!(git(&["status", "--porcelain"]), b"");
}
