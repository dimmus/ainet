use std::path::{Path, PathBuf};

use ainet_core::protocol::SeatSpec;
use ainet_core::{Edge, EdgeKind, Runtime};
use ainet_spec::{
    ResolvedSeat, launch_order, library, load, load_named, parse, seat_from_spec, to_yaml, validate,
};

/// A fresh scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ainet-spec-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn seat<'a>(seats: &'a [ResolvedSeat], logical: &str) -> &'a ResolvedSeat {
    seats
        .iter()
        .find(|s| s.logical == logical)
        .unwrap_or_else(|| panic!("no seat {logical}"))
}

const BUILTINS: [(&str, usize); 6] = [
    ("starter", 2),
    ("workshop", 4),
    ("factory", 7),
    ("code-review", 3),
    ("research", 3),
    ("pm", 3),
];

#[test]
fn builtins_resolve() {
    let cwd = std::env::current_dir().unwrap();
    let listed = library();
    for (name, count) in BUILTINS {
        let rig = load_named(name).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(rig.name, name);
        assert_eq!(rig.seats.len(), count, "{name}");
        assert!(rig.summary.is_some(), "{name}");
        assert!(rig.spec_path.is_none());
        assert!(rig.warnings.is_empty(), "{name}: {:?}", rig.warnings);
        for s in &rig.seats {
            assert_eq!(s.runtime, Runtime::ClaudeCode);
            assert_eq!(s.cwd, cwd);
            assert_eq!(s.address, format!("{}-{}@{name}", s.pod, s.member));
            assert!(s.agent_name.is_some());
            assert_eq!(s.guidance.len(), 2, "{name} {}: culture and role", s.logical);
            assert!(s.guidance[0].text.contains("reply_message"));
            assert!(s.send_text.is_empty());
        }
        let entry = listed.iter().find(|e| e.name == name).unwrap();
        if entry.source == "builtin" {
            assert_eq!(entry.seats, count);
        }
        assert!(ainet_spec::spec_yaml(name).unwrap().contains("version:"));
        assert!(validate(Path::new(name)).is_ok());
    }
}

#[test]
fn builtin_launch_order_follows_delegation() {
    let rig = load_named("factory").unwrap();
    let pos = |id: &str| rig.seats.iter().position(|s| s.logical == id).unwrap();
    assert_eq!(pos("orch.lead"), 0);
    assert!(pos("dev.build") < pos("dev.qa"));
    let review = load_named("starter").unwrap();
    assert_eq!(review.seats[0].logical, "dev.build");
}

fn fake(logical: &str) -> ResolvedSeat {
    let (pod, member) = logical.split_once('.').unwrap();
    ResolvedSeat {
        pod: pod.into(),
        member: member.into(),
        logical: logical.into(),
        address: format!("{pod}-{member}@t"),
        label: None,
        role: None,
        runtime: Runtime::ClaudeCode,
        model: None,
        effort: None,
        cwd: PathBuf::from("/"),
        restore_policy: "resume_if_possible".into(),
        agent_name: None,
        guidance: Vec::new(),
        send_text: Vec::new(),
        restore_send_text: Vec::new(),
        skills: Vec::new(),
    }
}

fn edge(kind: EdgeKind, from: &str, to: &str) -> Edge {
    Edge {
        kind,
        from: from.into(),
        to: to.into(),
    }
}

#[test]
fn launch_order_sorts_and_reports_cycles() {
    let seats: Vec<_> = ["p.a", "p.b", "p.c", "p.d"].map(fake).into();
    // b delegates to a; c was spawned by d; collaboration doesn't constrain.
    let edges = vec![
        edge(EdgeKind::DelegatesTo, "p.b", "p.a"),
        edge(EdgeKind::SpawnedBy, "p.c", "p.d"),
        edge(EdgeKind::CollaboratesWith, "p.a", "p.b"),
    ];
    assert_eq!(launch_order(&seats, &edges).unwrap(), vec![1, 0, 3, 2]);
    assert_eq!(launch_order(&seats, &[]).unwrap(), vec![0, 1, 2, 3]);

    let cycle = vec![
        edge(EdgeKind::DelegatesTo, "p.a", "p.b"),
        edge(EdgeKind::DelegatesTo, "p.b", "p.c"),
        edge(EdgeKind::SpawnedBy, "p.a", "p.c"),
    ];
    let err = launch_order(&seats, &cycle).unwrap_err().to_string();
    assert!(err.contains("cycle"), "{err}");
    for s in ["p.a", "p.b", "p.c"] {
        assert!(err.contains(s), "{err}");
    }
    assert!(!err.contains("p.d"), "{err}");

    let selfloop = vec![edge(EdgeKind::DelegatesTo, "p.d", "p.d")];
    assert!(
        launch_order(&seats, &selfloop)
            .unwrap_err()
            .to_string()
            .contains("p.d -> p.d")
    );
}

const TWO_PODS: &str = r#"
version: "0.2"
name: demo
pods:
  - id: orch
    label: Orchestration
    members:
      - id: lead
        runtime: claude-code
        cwd: "."
  - id: dev
    label: Dev
    members:
      - id: build
        runtime: codex
        model: gpt-5
        cwd: "."
      - id: qa
        runtime: claude-code
        model: claude-opus-4-6
        effort: high
        restore_policy: relaunch_fresh
        cwd: "."
      - id: shell
        agent_ref: builtin:terminal
        profile: none
        runtime: terminal
        cwd: "."
        startup:
          actions:
            - type: send_text
              value: tail -f log.txt
              idempotent: true
              applies_on: [fresh_start]
    edges:
      - kind: delegates_to
        from: build
        to: qa
edges:
  - kind: delegates_to
    from: orch.lead
    to: dev.build
  - kind: can_observe
    from: orch.lead
    to: dev.qa
"#;

#[test]
fn edges_are_qualified() {
    let dir = scratch("edges");
    let rig = parse(TWO_PODS, &dir, None).unwrap();
    assert_eq!(
        rig.edges,
        vec![
            edge(EdgeKind::DelegatesTo, "dev.build", "dev.qa"),
            edge(EdgeKind::DelegatesTo, "orch.lead", "dev.build"),
            edge(EdgeKind::CanObserve, "orch.lead", "dev.qa"),
        ]
    );
    let order: Vec<&str> = rig.seats.iter().map(|s| s.logical.as_str()).collect();
    assert_eq!(order, ["orch.lead", "dev.build", "dev.qa", "dev.shell"]);
    let shell = seat(&rig.seats, "dev.shell");
    assert_eq!(shell.runtime, Runtime::Shell);
    assert_eq!(shell.send_text, ["tail -f log.txt"]);
    assert!(shell.restore_send_text.is_empty());
}

#[test]
fn export_round_trips() {
    let dir = scratch("export");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let yaml = TWO_PODS.replacen(
        "model: gpt-5\n        cwd: \".\"",
        "model: gpt-5\n        cwd: sub",
        1,
    );
    let mut rig = parse(&yaml, &dir, None).unwrap();
    rig.summary = Some("A demo: with a colon".into());
    let exported = to_yaml(&rig);
    assert!(exported.contains("from: build"), "{exported}");
    assert!(exported.contains("from: orch.lead"), "{exported}");
    let back = parse(&exported, &dir, None).unwrap_or_else(|e| panic!("{e:#}\n{exported}"));
    assert_eq!(back.name, rig.name);
    assert_eq!(back.summary, rig.summary);
    assert_eq!(back.edges, rig.edges);
    assert_eq!(back.seats.len(), rig.seats.len());
    for (a, b) in rig.seats.iter().zip(&back.seats) {
        assert_eq!(a.logical, b.logical);
        assert_eq!(a.runtime, b.runtime);
        assert_eq!(a.model, b.model);
        assert_eq!(a.effort, b.effort);
        assert_eq!(a.cwd, b.cwd);
        assert_eq!(a.restore_policy, b.restore_policy);
    }

    // A built-in's agents survive the trip through the library.
    let starter = load_named("starter").unwrap();
    let exported = to_yaml(&starter);
    assert!(exported.contains("culture_file: CULTURE.md"), "{exported}");
    let again = parse(&exported, &dir, None).unwrap();
    assert_eq!(again.seats[1].agent_name.as_deref(), Some("reviewer"));
    assert_eq!(again.seats[1].guidance, starter.seats[1].guidance);
    assert!(
        again.warnings.iter().any(|w| w.contains("built-in agent")),
        "{:?}",
        again.warnings
    );

    // Another spec's agent that shares a built-in's name is not mistaken for it.
    write(
        &dir,
        "agents/reviewer/agent.yaml",
        "name: reviewer\nversion: \"1.0\"\n",
    );
    let mine = parse(
        &TWO_PODS.replacen(
            "runtime: codex",
            "runtime: codex\n        agent_ref: local:agents/reviewer",
            1,
        ),
        &dir,
        None,
    )
    .unwrap();
    assert_eq!(
        seat(&mine.seats, "dev.build").agent_name.as_deref(),
        Some("reviewer")
    );
    assert!(!to_yaml(&mine).contains("agent_ref: local:agents/reviewer"));
}

fn errors_of(dir: &Path, yaml: &str) -> Vec<String> {
    write(dir, "rig.yaml", yaml);
    let report = validate(dir);
    assert!(load(dir).is_err() == !report.errors.is_empty());
    report.errors
}

fn has(errors: &[String], needle: &str) -> bool {
    errors.iter().any(|e| e.contains(needle))
}

#[test]
fn validation_reports_bad_specs() {
    let dir = scratch("invalid");
    let errors = errors_of(
        &dir,
        r#"
version: "0.2"
name: bad
culture_file: ../CULTURE.md
pods:
  - id: dev.x
    label: X
    members: []
  - id: dev
    label: Dev
    members:
      - id: a@b
        runtime: claude-code
        cwd: "."
      - id: build
        runtime: claude-code
        cwd: "."
        startup:
          files:
            - path: /etc/passwd
      - id: build
        runtime: claude-code
        cwd: "."
      - id: pi
        runtime: pi
        cwd: "."
      - id: term
        runtime: terminal
        profile: default
        cwd: "."
      - id: refs
        agent_ref: "agents/x"
        cwd: "."
    edges:
      - kind: delegates_to
        from: build
        to: ghost
      - kind: bosses
        from: build
        to: pi
      - kind: delegates_to
        from: dev.build
        to: pi
  - id: dev
    label: Again
    members: []
edges:
  - kind: delegates_to
    from: dev.build
    to: dev.pi
  - kind: delegates_to
    from: build
    to: nowhere.x
"#,
    );
    for needle in [
        "pod id \"dev.x\" must not contain",
        "\"a@b\" must not contain",
        "member id is used twice",
        "pod id \"dev\" is used twice",
        "culture_file: file path \"../CULTURE.md\" must not contain `..`",
        "file path \"/etc/passwd\" must be relative",
        "seat dev.pi: runtime pi is not supported by ainet",
        "seat dev.term: a terminal node needs",
        "agent_ref \"agents/x\" must start with local:",
        "\"ghost\" is not a member of pod dev",
        "edge kind \"bosses\"",
        "pod-local edges use member ids",
        "both ends are in pod dev",
        "cross-pod edges use `pod.member` ids",
        "no seat \"nowhere.x\"",
    ] {
        assert!(has(&errors, needle), "missing {needle:?} in {errors:#?}");
    }

    let err = load(&dir).unwrap_err().to_string();
    assert!(err.contains("runtime pi is not supported"), "{err}");
}

#[test]
fn validation_reports_cycles_and_startup_rules() {
    let dir = scratch("cycle");
    let errors = errors_of(
        &dir,
        r#"
version: "0.2"
name: loop
startup:
  actions:
    - type: shell
      value: rm -rf /
      idempotent: true
    - type: send_text
      value: hi
      idempotent: false
    - type: send_text
      value: no flag
pods:
  - id: p
    label: P
    members:
      - id: a
        runtime: claude-code
        cwd: "."
      - id: b
        runtime: claude-code
        cwd: "."
    edges:
      - kind: delegates_to
        from: a
        to: b
      - kind: spawned_by
        from: a
        to: b
"#,
    );
    assert!(has(&errors, "shell actions are not supported"), "{errors:#?}");
    assert!(
        has(&errors, "non-idempotent action must not apply on restore"),
        "{errors:#?}"
    );
    assert!(has(&errors, "`idempotent` is required"), "{errors:#?}");

    let errors = errors_of(
        &dir,
        "name: loop\npods:\n  - id: p\n    members:\n      - id: a\n      - id: b\n    edges:\n      - {kind: delegates_to, from: a, to: b}\n      - {kind: spawned_by, from: a, to: b}\n",
    );
    assert!(
        has(&errors, "cycle: p.a -> p.b -> p.a") || has(&errors, "cycle: p.b -> p.a -> p.b"),
        "{errors:#?}"
    );
    let report = validate(&dir);
    assert!(report.warnings.iter().any(|w| w.contains("version")), "{report}");
    assert!(report.to_string().ends_with("invalid: 1 error"), "{report}");
}

#[test]
fn ignored_fields_are_warnings() {
    let dir = scratch("warnings");
    write(
        &dir,
        "rig.yaml",
        r#"
version: "0.2"
name: warn
services:
  kind: compose
  compose_file: dc.yaml
permission_policy: builtin:yolo
docs:
  - path: README.md
pods:
  - id: p
    label: P
    continuity_policy: {enabled: true}
    members:
      - id: a
        runtime: claude-code
        cwd: "."
        codex_config_profile: x
        frobnicate: 1
"#,
    );
    let rig = load(&dir).unwrap();
    let w = rig.warnings.join("\n");
    for needle in [
        "`services`",
        "`permission_policy`",
        "`continuity_policy`",
        "`codex_config_profile`",
        "unknown key `frobnicate`",
    ] {
        assert!(w.contains(needle), "missing {needle} in {w}");
    }
    assert!(!w.contains("docs"), "{w}");
    assert!(validate(&dir).is_ok());
}

#[test]
fn cwd_resolution() {
    let dir = scratch("cwd");
    std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let yaml = |cwd: &str| {
        format!(
            "version: \"0.2\"\nname: c\npods:\n  - id: p\n    label: P\n    members:\n      - id: a\n        runtime: claude-code\n        cwd: \"{cwd}\"\n"
        )
    };
    let cwd = |c: &str| parse(&yaml(c), &dir, None).unwrap().seats[0].cwd.clone();
    assert_eq!(cwd("."), dir);
    assert_eq!(cwd("sub/deeper/.."), dir.join("sub"));
    assert_eq!(cwd("~"), home);
    assert_eq!(cwd("$HOME"), home);
    assert_eq!(cwd("${HOME}/x"), home.join("x"));
    assert_eq!(cwd("/tmp"), PathBuf::from("/tmp"));
    assert_eq!(
        cwd("$AINET_SURELY_UNSET_VAR/x"),
        dir.join("$AINET_SURELY_UNSET_VAR/x")
    );
}

#[test]
fn agent_specs_supply_defaults_and_startup() {
    let dir = scratch("agents");
    write(
        &dir,
        "agents/impl/agent.yaml",
        r#"
name: impl
version: "1.0"
defaults:
  runtime: codex
  model: gpt-5
  effort: high
  lifecycle:
    restore_policy: relaunch_fresh
imports:
  - ref: local:../shared
profiles:
  default:
    uses:
      skills: [debugging, shared:tdd]
      guidance: [role]
      plugins: [shared:core]
  quick:
    preferences:
      model: gpt-5-mini
resources:
  guidance:
    - id: role
      path: guidance/role.md
    - id: intro
      path: guidance/intro.md
  skills:
    - id: debugging
      path: skills/debugging
startup:
  files:
    - path: guidance/intro.md
      delivery_hint: send_text
    - path: guidance/notes.txt
      required: false
  actions:
    - type: send_text
      value: begin
      idempotent: true
      applies_on: [fresh_start]
    - type: slash_command
      value: /status
      idempotent: true
"#,
    );
    write(&dir, "agents/impl/guidance/role.md", "You implement.");
    write(&dir, "agents/impl/guidance/intro.md", "Hello, builder.");
    write(&dir, "agents/impl/skills/debugging/SKILL.md", "# debugging");
    write(
        &dir,
        "agents/shared/agent.yaml",
        "name: shared\nversion: \"1.0\"\nresources:\n  skills:\n    - id: tdd\n      path: skills/tdd\n",
    );
    write(&dir, "agents/shared/skills/tdd/SKILL.md", "# tdd");
    write(&dir, "CULTURE.md", "Be kind.");
    write(&dir, "pod.md", "Pod rules.");
    write(&dir, "kick.txt", "Read the brief.");
    write(
        &dir,
        "rig.yaml",
        r#"
version: "0.2"
name: agents
culture_file: CULTURE.md
pods:
  - id: dev
    label: Dev
    startup:
      files:
        - path: pod.md
    members:
      - id: build
        agent_ref: local:agents/impl
        profile: default
        cwd: "."
        startup:
          files:
            - path: kick.txt
              applies_on: [restore]
      - id: fast
        agent_ref: local:agents/impl
        profile: quick
        runtime: claude-code
        restore_policy: resume_if_possible
        cwd: "."
"#,
    );
    let rig = load(&dir).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(rig.spec_path.as_deref(), Some(dir.join("rig.yaml").as_path()));
    let build = seat(&rig.seats, "dev.build");
    assert_eq!(build.runtime, Runtime::Codex);
    assert_eq!(build.model.as_deref(), Some("gpt-5"));
    assert_eq!(build.effort.as_deref(), Some("high"));
    assert_eq!(build.restore_policy, "relaunch_fresh");
    assert_eq!(build.agent_name.as_deref(), Some("impl"));
    let texts: Vec<&str> = build.guidance.iter().map(|d| d.text.as_str()).collect();
    assert_eq!(texts, ["Be kind.", "Pod rules.", "You implement."]);
    assert_eq!(build.send_text, ["Hello, builder.", "begin", "/status"]);
    assert_eq!(
        build.restore_send_text,
        ["Hello, builder.", "Read the brief.", "/status"]
    );
    assert_eq!(
        build.skills,
        [
            dir.join("agents/impl/skills/debugging"),
            dir.join("agents/shared/skills/tdd")
        ]
    );

    let fast = seat(&rig.seats, "dev.fast");
    assert_eq!(fast.runtime, Runtime::ClaudeCode);
    assert_eq!(fast.model.as_deref(), Some("gpt-5-mini"));
    assert_eq!(
        fast.restore_policy, "relaunch_fresh",
        "a member cannot widen the agent's policy"
    );
    let w = rig.warnings.join("\n");
    assert!(w.contains("guidance/notes.txt not found"), "{w}");
    assert!(w.contains("plugins"), "{w}");
    assert!(w.contains("cannot widen"), "{w}");

    // A required startup file that is missing fails the load.
    std::fs::remove_file(dir.join("kick.txt")).unwrap();
    let err = format!("{:#}", load(&dir).unwrap_err());
    assert!(err.contains("kick.txt not found"), "{err}");

    // An unknown profile and a missing agent are errors.
    let rig_yaml = std::fs::read_to_string(dir.join("rig.yaml")).unwrap();
    write(&dir, "kick.txt", "Read the brief.");
    write(
        &dir,
        "rig.yaml",
        &rig_yaml.replace("profile: quick", "profile: nope").replace(
            "local:agents/impl\n        profile: default",
            "local:agents/none\n        profile: default",
        ),
    );
    let report = validate(&dir);
    assert!(has(&report.errors, "has no profile \"nope\""), "{report}");
    assert!(has(&report.errors, "no agent.yaml"), "{report}");
}

#[test]
fn seats_from_seat_specs() {
    let dir = scratch("seatspec");
    write(
        &dir,
        "agents/a/agent.yaml",
        "name: a\nversion: \"1.0\"\ndefaults:\n  model: opus\nresources:\n  guidance:\n    - id: role\n      path: role.md\n",
    );
    write(&dir, "agents/a/role.md", "Role A.");
    let spec = SeatSpec {
        pod: "dev".into(),
        member: "extra".into(),
        agent_ref: Some("local:agents/a".into()),
        guidance: Some("Be brief.".into()),
        first_message: Some("Start.".into()),
        ..SeatSpec::default()
    };
    let s = seat_from_spec("grown", &dir, &spec).unwrap();
    assert_eq!(s.address, "dev-extra@grown");
    assert_eq!(s.runtime, Runtime::ClaudeCode);
    assert_eq!(s.model.as_deref(), Some("opus"));
    assert_eq!(s.cwd, dir);
    let texts: Vec<&str> = s.guidance.iter().map(|d| d.text.as_str()).collect();
    assert_eq!(texts, ["Role A.", "Be brief."]);
    assert_eq!(s.send_text, ["Start."]);

    let shell = SeatSpec {
        pod: "ops".into(),
        member: "tail".into(),
        runtime: Some(Runtime::Shell),
        ..SeatSpec::default()
    };
    assert_eq!(
        seat_from_spec("grown", &dir, &shell).unwrap().runtime,
        Runtime::Shell
    );
    let bad = SeatSpec {
        pod: "a.b".into(),
        member: "x".into(),
        ..SeatSpec::default()
    };
    assert!(seat_from_spec("grown", &dir, &bad).is_err());
}

/// OpenRig's own specs, when a clone is around: every rig that uses only
/// runtimes ainet runs must resolve.
#[test]
fn openrig_specs_resolve() {
    // Point AINET_OPENRIG_SPECS at <openrig clone>/packages/daemon/specs/rigs.
    let Some(root) = std::env::var_os("AINET_OPENRIG_SPECS").map(PathBuf::from) else {
        eprintln!("skipping: set AINET_OPENRIG_SPECS to an OpenRig clone's packages/daemon/specs/rigs");
        return;
    };
    if !root.is_dir() {
        eprintln!("skipping: no OpenRig specs at {}", root.display());
        return;
    }
    let mut files = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "rig.yaml") {
                files.push(path);
            }
        }
    }
    assert!(!files.is_empty());
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let unsupported = ["pi", "omp", "stub"]
            .iter()
            .any(|r| text.contains(&format!("runtime: {r}\n")));
        match load(&file) {
            Ok(rig) => {
                assert!(!rig.seats.is_empty(), "{}", file.display());
                for s in &rig.seats {
                    if s.runtime != Runtime::Shell {
                        assert!(s.agent_name.is_some(), "{} {}", file.display(), s.logical);
                    }
                }
            }
            Err(e) => assert!(unsupported, "{}: {e:#}", file.display()),
        }
    }
}
