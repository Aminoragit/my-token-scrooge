#![forbid(unsafe_code)]

mod cli;
mod state;
mod tui;

use clap::Parser;
use cli::*;
use mts_core::{
    CompiledPolicy, Decision, ErrorBounds, FanoutTransaction, FileUpdate, Operation, PolicySet,
    ReadBounds, RuleScope, SearchBounds, ShellFamily, bounded_metadata, bounded_read,
    bounded_search, content_hash, extract_error_regions, extract_shell_intents,
};
use mts_harnesses::{
    DetectionEvidence, Target, detect_targets, registry, support_matrix_markdown, target_by_id,
};
use serde_json::json;
use state::{
    EnforcementMode, Store, atomic_write, ensure_layout, load_config, mts_home, save_config,
};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MINIMAL_FULL: &str = include_str!("../../../presets/minimal/block-full.txt");
const MINIMAL_PARTIAL: &str = include_str!("../../../presets/minimal/block-partial.txt");
const BALANCED_FULL: &str = include_str!("../../../presets/balanced/block-full.txt");
const BALANCED_PARTIAL: &str = include_str!("../../../presets/balanced/block-partial.txt");
const STRICT_FULL: &str = include_str!("../../../presets/strict/block-full.txt");
const STRICT_PARTIAL: &str = include_str!("../../../presets/strict/block-partial.txt");
const MTS_CODEX_HOOK_STATUS: &str = "Checking my-token-scrooge policy";
const MTS_CODEX_HOOK_COMMAND: &str = "mts hook codex-cli";
const MTS_CLAUDE_HOOK_COMMAND: &str = "mts hook claude-code-cli";
const MTS_ANTIGRAVITY_HOOK_COMMAND: &str = "mts hook antigravity-cli";
const MTS_ANTIGRAVITY_HOOK_NAME: &str = "my-token-scrooge";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeHookProvider {
    Codex,
    Claude,
    Antigravity,
}

impl NativeHookProvider {
    fn for_target(target: &str) -> Option<Self> {
        match target {
            "codex-cli" => Some(Self::Codex),
            "claude-code-cli" => Some(Self::Claude),
            "antigravity-cli" => Some(Self::Antigravity),
            _ => None,
        }
    }

    const fn target_id(self) -> &'static str {
        match self {
            Self::Codex => "codex-cli",
            Self::Claude => "claude-code-cli",
            Self::Antigravity => "antigravity-cli",
        }
    }

    const fn command(self) -> &'static str {
        match self {
            Self::Codex => MTS_CODEX_HOOK_COMMAND,
            Self::Claude => MTS_CLAUDE_HOOK_COMMAND,
            Self::Antigravity => MTS_ANTIGRAVITY_HOOK_COMMAND,
        }
    }

    const fn manifest_key(self) -> &'static str {
        match self {
            Self::Codex => "codex_hooks",
            Self::Claude => "claude_hooks",
            Self::Antigravity => "antigravity_hooks",
        }
    }
}

fn main() {
    if let Err(message) = run(Cli::parse()) {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let home = mts_home();
    if let Some(Command::Setup(args)) = &cli.command
        && args.dry_run
    {
        return setup_preview(&home, args);
    }
    ensure_layout(&home).map_err(|error| format!("MTS_HOME_ERROR: {error}"))?;
    let store = Store::open(&home).map_err(store_error)?;
    match cli.command {
        None => {
            let config = load_config(&home)?;
            let installed = store.installed_targets().map_err(store_error)?;
            tui::run(&home, config.mode.as_str(), &installed)
                .map_err(|error| format!("MTS_TUI_ERROR: {error}"))
        }
        Some(Command::Setup(args)) => setup(&home, &store, args),
        Some(Command::Doctor) => doctor(&home, &store),
        Some(Command::Status) => status(&home, &store),
        Some(Command::Uninstall(args)) => uninstall(&home, &store, args),
        Some(Command::Mode(args)) => mode(&home, args),
        Some(Command::Project(args)) => project(args),
        Some(Command::Policy(args)) => policy(&home, &store, args),
        Some(Command::Harness(args)) => harness(&home, &store, args),
        Some(Command::Simulate(args)) => simulate(&home, &store, args),
        Some(Command::Retries(args)) => retries(&store, args),
        Some(Command::AllowOnce(args)) => allow_once(&store, args),
        Some(Command::Savings(args)) => savings(&store, args),
        Some(Command::Report(args)) => report(&store, args),
        Some(Command::Benchmark(args)) => benchmark(args),
        Some(Command::Hook(args)) => hook_dispatch(&home, &store, args),
        Some(Command::AcpProxy(args)) | Some(Command::Wrapper(args)) => passthrough(args),
        Some(Command::Worker(args)) => worker(&home, args),
        Some(Command::RepoBootstrap(_)) => init_project(Path::new("."), "overlay"),
        Some(Command::SelfManage(args)) => self_manage(&home, args),
    }
}

fn setup_preview(home: &Path, args: &SetupArgs) -> Result<(), String> {
    if let Some(CustomSetup::Custom {
        id, command, mode, ..
    }) = &args.custom
    {
        validate_custom(id, command, mode)?;
        println!("Create: {}", home.join("harnesses").join(id).display());
        println!("Dry run complete; no files changed.");
        return Ok(());
    }
    preset(&args.profile)?;
    let targets = if args.targets.is_empty() {
        detect_installed()
    } else if args.targets == ["all"] {
        registry().iter().collect()
    } else {
        args.targets
            .iter()
            .map(String::as_str)
            .map(resolve_target)
            .collect::<Result<Vec<_>, _>>()?
    };
    if targets.is_empty() {
        return Err(
            "MTS_SETUP_NO_TARGETS: no harness was detected. Retry with --targets <target-id>."
                .into(),
        );
    }
    println!("Setup preview (mode: SHADOW, profile: {}):", args.profile);
    for target in targets {
        let directory = home.join("harnesses").join(target.id);
        for name in [
            "block-full.txt",
            "block-partial.txt",
            "adapter.json",
            "install-manifest.json",
        ] {
            println!("  Create or update: {}", directory.join(name).display());
        }
        if let Some(provider) = NativeHookProvider::for_target(target.id) {
            println!(
                "  Merge MTS handler into: {}",
                hook_config_path(provider, args.codex_home.as_deref()).display()
            );
        }
    }
    println!("Dry run complete; no files changed.");
    Ok(())
}

fn setup(home: &Path, store: &Store, args: SetupArgs) -> Result<(), String> {
    let hook_root_override = args.codex_home.clone();
    if let Some(CustomSetup::Custom {
        id,
        command,
        workspace_arg,
        mode,
    }) = args.custom
    {
        return setup_custom(
            home,
            store,
            &id,
            &command,
            workspace_arg.as_deref(),
            &mode,
            args.dry_run,
        );
    }
    let (full, partial) = preset(&args.profile)?;
    let full_canonical = canonical(full);
    let partial_canonical = canonical(partial);
    CompiledPolicy::parse_full(full).map_err(|error| error.to_string())?;
    CompiledPolicy::parse_partial(partial).map_err(|error| error.to_string())?;
    let targets = if args.targets.is_empty() {
        detect_installed()
    } else if args.targets == ["all"] {
        registry().iter().collect()
    } else {
        args.targets
            .iter()
            .map(String::as_str)
            .map(resolve_target)
            .collect::<Result<Vec<_>, _>>()?
    };
    if targets.is_empty() {
        return Err(
            "MTS_SETUP_NO_TARGETS: no harness was detected. Retry with --targets <target-id>."
                .into(),
        );
    }
    let hook_plans = targets
        .iter()
        .filter_map(|target| NativeHookProvider::for_target(target.id))
        .map(|provider| HookPlan::load(home, provider, hook_root_override.as_deref()))
        .collect::<Result<Vec<_>, _>>()?;
    println!("Setup preview (mode: SHADOW, profile: {}):", args.profile);
    for target in &targets {
        let directory = home.join("harnesses").join(target.id);
        for name in [
            "block-full.txt",
            "block-partial.txt",
            "adapter.json",
            "install-manifest.json",
        ] {
            println!("  Create or update: {}", directory.join(name).display());
        }
        if let Some(provider) = NativeHookProvider::for_target(target.id) {
            println!(
                "  Merge MTS handler into: {}",
                hook_plans
                    .iter()
                    .find(|plan| plan.provider == provider)
                    .expect("hook plan exists for native-hook target")
                    .path
                    .display()
            );
        }
    }
    if args.dry_run {
        println!("Dry run complete; no files changed.");
        return Ok(());
    }
    if !args.yes && !confirm("Apply this setup transaction? [y/N] ")? {
        return Err("MTS_SETUP_CANCELLED: no files changed.".into());
    }
    for parent in hook_plans.iter().filter_map(|plan| plan.path.parent()) {
        fs::create_dir_all(parent).map_err(io_error("MTS_HOOKS_DIRECTORY"))?;
    }
    let mut config = load_config(home)?;
    let original_config = fs::read(home.join("config.toml")).ok();
    let transaction_id = timestamp();
    let backup_root = home.join("backups").join(&transaction_id);
    let mut updates = Vec::new();
    let mut records = Vec::new();
    let mut target_ids = Vec::new();
    for target in targets {
        target_ids.push(target.id.to_string());
        let directory = home.join("harnesses").join(target.id);
        fs::create_dir_all(&directory).map_err(io_error("MTS_SETUP_DIRECTORY"))?;
        backup_owned_files(&directory, &backup_root.join(target.id))?;
        let capabilities = capability_json(target);
        let paths = json!([
            directory.join("block-full.txt"),
            directory.join("block-partial.txt")
        ])
        .to_string();
        let mut manifest = json!({
            "schema_version": 1,
            "target_id": target.id,
            "adapter_version": env!("CARGO_PKG_VERSION"),
            "mode": "SHADOW",
            "profile": args.profile,
            "installed_at": transaction_id,
            "contract": "UNVERIFIED",
            "policy_hashes": {
                "block-full.txt": content_hash(full_canonical.as_bytes()),
                "block-partial.txt": content_hash(partial_canonical.as_bytes())
            },
            "owned_files": [
                "block-full.txt", "block-partial.txt", "adapter.json", "install-manifest.json"
            ]
        });
        if let Some(provider) = NativeHookProvider::for_target(target.id) {
            let plan = hook_plans
                .iter()
                .find(|plan| plan.provider == provider)
                .expect("hook plan exists for native-hook target");
            manifest[provider.manifest_key()] = json!({
                "path": plan.path,
                "created_by_mts": plan.created_by_mts,
                "installed_hash": content_hash(&plan.contents)
            });
        }
        updates.extend([
            update_for(
                directory.join("block-full.txt"),
                full_canonical.clone().into_bytes(),
            )?,
            update_for(
                directory.join("block-partial.txt"),
                partial_canonical.clone().into_bytes(),
            )?,
            update_for(
                directory.join("adapter.json"),
                pretty_json(&capabilities)?.into_bytes(),
            )?,
            update_for(
                directory.join("install-manifest.json"),
                pretty_json(&manifest)?.into_bytes(),
            )?,
        ]);
        records.push((target.id.to_string(), capabilities.to_string(), paths));
    }
    for plan in &hook_plans {
        updates.push(update_for(plan.path.clone(), plan.contents.clone())?);
    }
    config.mode = EnforcementMode::Shadow;
    config.profile = args.profile;
    save_config(home, &config).map_err(io_error("MTS_CONFIG_WRITE"))?;
    if let Err(error) = FanoutTransaction::commit(updates, validate_candidate) {
        restore_config(home, original_config.as_deref())?;
        return Err(error.to_string());
    }
    if let Err(error) = store.record_installations(&records) {
        rollback_setup_files(home, &backup_root, &target_ids)?;
        restore_config(home, original_config.as_deref())?;
        for plan in &hook_plans {
            restore_optional_file(&plan.path, plan.original.as_deref())?;
        }
        return Err(store_error(error));
    }
    println!("Setup complete in SHADOW mode. Run mts doctor before promotion.");
    Ok(())
}

fn setup_custom(
    home: &Path,
    store: &Store,
    id: &str,
    command: &str,
    workspace_arg: Option<&str>,
    mode: &str,
    dry_run: bool,
) -> Result<(), String> {
    validate_custom(id, command, mode)?;
    let directory = home.join("harnesses").join(id);
    println!("Create: {}", directory.display());
    if dry_run {
        return Ok(());
    }
    fs::create_dir_all(&directory).map_err(io_error("MTS_CUSTOM_DIRECTORY"))?;
    let backup_root = home.join("backups").join(timestamp());
    backup_owned_files(&directory, &backup_root.join(id))?;
    let adapter = toml::to_string_pretty(&json!({
        "id": id,
        "command": command,
        "workspace_arg": workspace_arg,
        "mode": mode,
        "default_protection": "SHADOW"
    }))
    .map_err(|error| format!("MTS_CUSTOM_TOML: {error}"))?;
    let manifest = json!({
        "schema_version": 1,
        "target_id": id,
        "adapter_version": env!("CARGO_PKG_VERSION"),
        "mode": "SHADOW",
        "contract": "UNVERIFIED",
        "policy_hashes": {
            "block-full.txt": content_hash(MINIMAL_FULL.as_bytes()),
            "block-partial.txt": content_hash(MINIMAL_PARTIAL.as_bytes())
        },
        "owned_files": ["adapter.toml", "block-full.txt", "block-partial.txt", "install-manifest.json"]
    });
    let updates = vec![
        update_for(directory.join("adapter.toml"), adapter.into_bytes())?,
        update_for(
            directory.join("block-full.txt"),
            MINIMAL_FULL.as_bytes().to_vec(),
        )?,
        update_for(
            directory.join("block-partial.txt"),
            MINIMAL_PARTIAL.as_bytes().to_vec(),
        )?,
        update_for(
            directory.join("install-manifest.json"),
            pretty_json(&manifest)?.into_bytes(),
        )?,
    ];
    FanoutTransaction::commit(updates, validate_candidate).map_err(|error| error.to_string())?;
    if let Err(error) =
        store.record_installation(id, r#"{"maximum":"PARTIAL","contract":"UNVERIFIED"}"#, "[]")
    {
        rollback_setup_files(home, &backup_root, &[id.to_string()])?;
        return Err(store_error(error));
    }
    println!("Custom target {id} installed in SHADOW mode.");
    Ok(())
}

fn validate_custom(id: &str, command: &str, mode: &str) -> Result<(), String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(
            "MTS_CUSTOM_ID: custom target IDs use lowercase letters, digits, and hyphens.".into(),
        );
    }
    if command.trim().is_empty() {
        return Err("MTS_CUSTOM_COMMAND: command must not be empty.".into());
    }
    if mode != "wrapper" {
        return Err("MTS_CUSTOM_MODE: v1 custom commands support wrapper mode.".into());
    }
    Ok(())
}

fn doctor(home: &Path, store: &Store) -> Result<(), String> {
    let config = load_config(home)?;
    let installed = store.installed_targets().map_err(store_error)?;
    println!("my-token-scrooge doctor\n\nCore");
    println!("  Policy engine              PASS");
    println!("  Event store                PASS");
    println!("  Mode                       {}", config.mode.as_str());
    println!("  Registered targets         {}", registry().len());
    if installed.is_empty() {
        println!("\nMTS_DOCTOR_NO_TARGETS: run mts setup --targets <target-id>.");
        return Ok(());
    }
    for id in installed {
        let directory = home.join("harnesses").join(&id);
        let policy_status = validate_policy_files(&directory)
            .map_or_else(|error| format!("INVALID ({error})"), |()| "VALID".into());
        println!("\n{id}");
        if let Some(target) = target_by_id(&id) {
            println!(
                "  Integration                {}",
                target
                    .families
                    .iter()
                    .map(|family| family.as_str())
                    .collect::<Vec<_>>()
                    .join(" + ")
            );
            println!("  Contract                   UNVERIFIED");
            println!("  Current grade              UNVERIFIED");
            println!("  Planned maximum            {}", target.capability.label());
            println!("  Recommended mode           SHADOW");
        } else {
            println!("  Integration                CUSTOM_COMMAND");
            println!("  Contract                   UNVERIFIED");
        }
        println!("  Policy files               {policy_status}");
        println!(
            "  Drift                      {}",
            if target_has_drift(&directory) {
                "DRIFT"
            } else {
                "CLEAN"
            }
        );
    }
    Ok(())
}

fn status(home: &Path, store: &Store) -> Result<(), String> {
    let config = load_config(home)?;
    let installed = store.installed_targets().map_err(store_error)?;
    println!(
        "Mode: {}\nProfile: {}\nInstalled targets: {}",
        config.mode.as_str(),
        config.profile,
        installed.len()
    );
    for target in installed {
        println!("- {target}");
    }
    Ok(())
}

fn uninstall(home: &Path, store: &Store, args: UninstallArgs) -> Result<(), String> {
    let targets = if args.targets.is_empty() {
        store.installed_targets().map_err(store_error)?
    } else {
        args.targets
    };
    for target in targets {
        remove_target(home, store, &target)?;
    }
    if args.purge {
        for directory in ["artifacts", "history", "backups", "logs"] {
            let path = home.join(directory);
            if path.exists() {
                fs::remove_dir_all(&path).map_err(io_error("MTS_PURGE_FAILED"))?;
            }
        }
    }
    println!("Uninstall complete. User-modified unknown files were preserved.");
    Ok(())
}

fn mode(home: &Path, args: ModeArgs) -> Result<(), String> {
    let mut config = load_config(home)?;
    let Some(value) = args.mode else {
        println!("{}", config.mode.as_str());
        return Ok(());
    };
    if value.eq_ignore_ascii_case("show") {
        println!("{}", config.mode.as_str());
        return Ok(());
    }
    let next = EnforcementMode::parse(&value)
        .ok_or_else(|| "MTS_MODE_INVALID: choose shadow, warn, enforce, or show.".to_string())?;
    if next == EnforcementMode::Enforce && config.mode == EnforcementMode::Shadow {
        return Err("MTS_MODE_PROMOTION: promote SHADOW to WARN before ENFORCE.".into());
    }
    config.mode = next;
    save_config(home, &config).map_err(io_error("MTS_CONFIG_WRITE"))?;
    println!("Mode changed to {}.", next.as_str());
    Ok(())
}

fn project(args: ProjectArgs) -> Result<(), String> {
    match args.command {
        ProjectCommand::Init { mode, path } => init_project(&path, &mode),
    }
}

fn init_project(path: &Path, mode: &str) -> Result<(), String> {
    if !matches!(mode, "global" | "overlay" | "isolated") {
        return Err("MTS_PROJECT_MODE: choose global, overlay, or isolated.".into());
    }
    let directory = path.join(".mts");
    fs::create_dir_all(&directory).map_err(io_error("MTS_PROJECT_CREATE"))?;
    let project = format!("mode = \"{mode}\"\nprofile = \"balanced\"\n");
    atomic_write(&directory.join("project.toml"), project.as_bytes())
        .map_err(io_error("MTS_PROJECT_WRITE"))?;
    for (name, contents) in [
        ("block-full.txt", BALANCED_FULL),
        ("block-partial.txt", BALANCED_PARTIAL),
    ] {
        if !directory.join(name).exists() {
            atomic_write(&directory.join(name), contents.as_bytes())
                .map_err(io_error("MTS_PROJECT_WRITE"))?;
        }
    }
    println!("Project policy initialized at {}.", directory.display());
    Ok(())
}

fn policy(home: &Path, store: &Store, args: PolicyArgs) -> Result<(), String> {
    match args.command {
        PolicyCommand::List(scope) => {
            for path in policy_paths(home, store, &scope.scope, scope.kind)? {
                println!("# {}", path.display());
                print!(
                    "{}",
                    fs::read_to_string(&path).map_err(io_error("MTS_POLICY_READ"))?
                );
            }
            Ok(())
        }
        PolicyCommand::Add { kind, rule, scope } => {
            mutate_policy(home, store, &scope, kind, |text| {
                add_rule(text, kind, &rule)
            })
        }
        PolicyCommand::Edit {
            kind,
            rule_id,
            replacement,
            scope,
        } => {
            let replacement = replacement.map_or_else(prompt_replacement, Ok)?;
            mutate_policy(home, store, &scope, kind, |text| {
                replace_rule(text, &rule_id, Some(&replacement))
            })
        }
        PolicyCommand::Remove {
            kind,
            rule_id,
            scope,
        } => mutate_policy(home, store, &scope, kind, |text| {
            replace_rule(text, &rule_id, None)
        }),
        PolicyCommand::Validate(scope) => {
            for path in policy_paths(home, store, &scope.scope, scope.kind)? {
                validate_one(&path)?;
                println!("VALID {}", path.display());
            }
            Ok(())
        }
        PolicyCommand::Format(scope) => {
            let paths = policy_paths(home, store, &scope.scope, scope.kind)?;
            let mut updates = Vec::new();
            for path in paths {
                let text = fs::read_to_string(&path).map_err(io_error("MTS_POLICY_READ"))?;
                validate_text(&path, &text)?;
                updates.push(update_for(path, format_policy(&text).into_bytes())?);
            }
            commit_policy_updates(updates)?;
            println!("Policy files formatted.");
            Ok(())
        }
    }
}

fn harness(home: &Path, store: &Store, args: HarnessArgs) -> Result<(), String> {
    match args.command {
        HarnessCommand::Detect => {
            let detected = detect_installed();
            if detected.is_empty() {
                println!("No registered command-line target detected.");
            }
            for target in detected {
                println!("{}\t{}", target.id, target.display_name);
            }
            Ok(())
        }
        HarnessCommand::List { format } if format == "markdown" => {
            print!("{}", support_matrix_markdown());
            Ok(())
        }
        HarnessCommand::List { format } if format == "table" => {
            for target in registry() {
                println!(
                    "{:<26} {:<20} {}",
                    target.id,
                    target.capability.label(),
                    target.adapter.default_mode.as_str()
                );
            }
            Ok(())
        }
        HarnessCommand::List { .. } => {
            Err("MTS_FORMAT: harness list supports table or markdown.".into())
        }
        HarnessCommand::Install { target } => setup(
            home,
            store,
            SetupArgs {
                profile: "balanced".into(),
                targets: vec![target],
                yes: true,
                dry_run: false,
                codex_home: None,
                custom: None,
            },
        ),
        HarnessCommand::Uninstall { target } => remove_target(home, store, &target),
        HarnessCommand::Verify { target } => verify_target(&target),
        HarnessCommand::Drift => drift(home, store),
        HarnessCommand::Sync { from, to } => sync_policy(home, store, &from, &to),
    }
}

fn simulate(home: &Path, store: &Store, args: SimulateArgs) -> Result<(), String> {
    let requested_operation = operation(args.operation);
    let policy = load_target_policy(home, &args.target)?;
    let (actual_operation, resource, query, line_range, shell_details, selected_decision) =
        if requested_operation == Operation::Shell {
            let family = if cfg!(windows) {
                ShellFamily::PowerShell
            } else {
                ShellFamily::Unix
            };
            let intents =
                extract_shell_intents(&args.input, family).map_err(|error| error.to_string())?;
            if intents.is_empty() {
                return Err(
                    "MTS_SHELL_NO_RESOURCE: no literal resource intent was found.".to_string(),
                );
            }
            let count = intents.len();
            let (selected, decision) = intents
                .into_iter()
                .map(|intent| {
                    let decision = policy.decide(intent.operation, &intent.resource);
                    (intent, decision)
                })
                .max_by_key(|(_, decision)| decision_rank(decision))
                .expect("non-empty shell intent list");
            (
                selected.operation,
                selected.resource,
                selected.search_query,
                selected.line_range,
                Some(count),
                Some(decision),
            )
        } else {
            (
                requested_operation,
                args.input.clone(),
                None,
                None,
                None,
                None,
            )
        };
    let decision = selected_decision.unwrap_or_else(|| policy.decide(actual_operation, &resource));
    let config = load_config(home)?;
    let output = decision_json(
        &decision,
        &args.target,
        actual_operation,
        &resource,
        config.mode,
        shell_details,
    );
    let output_text = pretty_json(&output)?;
    println!("{output_text}");
    let key = format!(
        "{}|{}|{}|{}|{}|{}",
        args.target,
        args.session,
        actual_operation.as_str(),
        mts_core::normalize_resource(&resource, cfg!(windows)),
        query.as_deref().unwrap_or(""),
        line_range
            .map(|(start, end)| format!("{start}-{end}"))
            .unwrap_or_default()
    );
    if !matches!(&decision, Decision::Allow) {
        let retry = store.record_retry(&key).map_err(store_error)?;
        let attempt = retry.1;
        let protected = fs::metadata(&resource)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map_or(0, |metadata| metadata.len());
        let decision_name = match &decision {
            Decision::Allow => "ALLOW",
            Decision::FullBlock(_) => "FULL_BLOCK",
            Decision::PartialBlock(_) => "PARTIAL_BLOCK",
        };
        let replacement = if retry.1 < 3 && config.mode == EnforcementMode::Enforce {
            match &decision {
                Decision::PartialBlock(partial) => {
                    run_substitute(&resource, partial, query.as_deref(), args.diff.as_deref())?
                }
                _ => 0,
            }
        } else {
            0
        };
        store
            .record_event(
                &args.target,
                actual_operation.as_str(),
                decision_name,
                &resource,
                protected,
                if config.mode == EnforcementMode::Enforce {
                    protected
                } else {
                    0
                },
                replacement,
                if attempt > 1 {
                    output_text.len() as u64
                } else {
                    0
                },
            )
            .map_err(store_error)?;
        if retry.1 >= 3 {
            println!("MTS_CIRCUIT_OPEN:{}", content_hash(key.as_bytes()));
            return Ok(());
        }
    }
    Ok(())
}

fn decision_rank(decision: &Decision) -> u8 {
    match decision {
        Decision::Allow => 0,
        Decision::PartialBlock(_) => 1,
        Decision::FullBlock(_) => 2,
    }
}

fn retries(store: &Store, args: RetriesArgs) -> Result<(), String> {
    match args.command {
        RetryCommand::List => {
            for (intent, state, attempts) in store.retry_rows().map_err(store_error)? {
                println!("{intent}\t{attempts}\t{state}");
            }
        }
        RetryCommand::Show { intent_id } => {
            let row = store
                .retry_rows()
                .map_err(store_error)?
                .into_iter()
                .find(|row| row.0 == intent_id)
                .ok_or_else(|| "MTS_RETRY_NOT_FOUND: unknown intent ID.".to_string())?;
            println!("intent={}\nattempts={}\nstate={}", row.0, row.2, row.1);
        }
        RetryCommand::Unlock { intent_id } => {
            store.clear_retry(&intent_id).map_err(store_error)?;
            println!("Retry circuit unlocked.");
        }
        RetryCommand::Lock { intent_id } => {
            store
                .set_retry_state(&intent_id, "CIRCUIT_OPEN")
                .map_err(store_error)?;
            println!("Retry circuit locked.");
        }
    }
    Ok(())
}

fn allow_once(store: &Store, args: AllowOnceArgs) -> Result<(), String> {
    let nonce = store
        .issue_approval(&args.session, &args.request_id, &args.operation)
        .map_err(store_error)?;
    println!("Approval nonce (single use, expires in 5 minutes): {nonce}");
    Ok(())
}

fn savings(store: &Store, _args: SavingsArgs) -> Result<(), String> {
    let (protected, avoided, replacement, retry, tokens) = store.savings().map_err(store_error)?;
    println!(
        "Protected bytes: {protected}\nAvoided output bytes: {avoided}\nReplacement output bytes: {replacement}\nRetry overhead bytes: {retry}\nEstimated net tokens saved: {tokens}\nMethod: recorded event estimates\nConfidence: per-event"
    );
    Ok(())
}

fn report(store: &Store, args: ReportArgs) -> Result<(), String> {
    let (protected, avoided, replacement, retry, tokens) = store.savings().map_err(store_error)?;
    match args.command {
        ReportCommand::Export { format } if format == "json" => {
            println!(
                "{}",
                pretty_json(&json!({
                    "protected_bytes": protected,
                    "avoided_output_bytes": avoided,
                    "replacement_output_bytes": replacement,
                    "retry_overhead_bytes": retry,
                    "estimated_net_tokens_saved": tokens
                }))?
            );
        }
        ReportCommand::Export { format } if format == "markdown" => {
            println!(
                "# MTS savings report\n\n| Metric | Value |\n|---|---:|\n| Protected bytes | {protected} |\n| Avoided output bytes | {avoided} |\n| Replacement output bytes | {replacement} |\n| Retry overhead bytes | {retry} |\n| Estimated net tokens saved | {tokens} |"
            );
        }
        _ => return Err("MTS_REPORT_FORMAT: choose markdown or json.".into()),
    }
    Ok(())
}

fn benchmark(args: BenchmarkArgs) -> Result<(), String> {
    match args.command {
        BenchmarkCommand::Run { suite } => {
            let suite = suite.unwrap_or_else(|| "smoke".into());
            if suite != "smoke" && suite != "dependency-read" {
                return Err("MTS_BENCHMARK_SUITE: unknown local suite.".into());
            }
            let mut policy = PolicySet::new();
            policy.add(
                CompiledPolicy::parse_full(BALANCED_FULL).map_err(|error| error.to_string())?,
                RuleScope::Harness,
            );
            policy.add(
                CompiledPolicy::parse_partial(BALANCED_PARTIAL)
                    .map_err(|error| error.to_string())?,
                RuleScope::Harness,
            );
            let read = policy.decide(Operation::Read, "node_modules/example/index.js");
            let edit = policy.decide(Operation::Edit, "node_modules/example/index.js");
            let passed =
                matches!(read, Decision::PartialBlock(_)) && matches!(edit, Decision::FullBlock(_));
            println!(
                "{}",
                pretty_json(&json!({
                    "suite": suite,
                    "reproducible": true,
                    "tasks": 2,
                    "passed": passed,
                    "external_ab_evidence": false
                }))?
            );
            if !passed {
                return Err("MTS_BENCHMARK_FAILED: policy smoke expectations changed.".into());
            }
        }
        BenchmarkCommand::Compare => {
            println!("No external baseline and protected runs are recorded.");
        }
        BenchmarkCommand::Export => {
            println!("Run data remains local. Use mts report export for recorded metrics.");
        }
    }
    Ok(())
}

fn hook_dispatch(home: &Path, store: &Store, args: PassthroughArgs) -> Result<(), String> {
    let target = args
        .arguments
        .get(usize::from(
            args.arguments
                .first()
                .is_some_and(|value| value == "dispatch"),
        ))
        .ok_or_else(|| "MTS_HOOK_TARGET: target ID is required.".to_string())?;
    let provider = NativeHookProvider::for_target(target);
    if !store
        .installed_targets()
        .map_err(store_error)?
        .iter()
        .any(|installed| installed == target)
    {
        let message =
            format!("MTS_HOOK_NOT_INSTALLED: target {target} has no installed physical policy.");
        if let Some(provider) = provider {
            println!(
                "{}",
                pretty_json(&provider_error_output(provider, &message))?
            );
            return Ok(());
        }
        return Err(message);
    }
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(io_error("MTS_HOOK_INPUT"))?;
    let payload: serde_json::Value = match serde_json::from_str(&input) {
        Ok(payload) => payload,
        Err(error) if provider.is_some() => {
            println!(
                "{}",
                pretty_json(&provider_error_output(
                    provider.expect("native provider exists"),
                    &format!("MTS_HOOK_JSON: {error}")
                ))?
            );
            return Ok(());
        }
        Err(error) => return Err(format!("MTS_HOOK_JSON: {error}")),
    };
    if let Some(provider) = provider {
        return native_hook_dispatch(home, store, target, provider, &payload);
    }
    let operation_text = payload
        .get("operation")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "MTS_HOOK_OPERATION: operation is required.".to_string())?;
    let operation = Operation::parse(operation_text)
        .ok_or_else(|| format!("MTS_HOOK_OPERATION: unsupported operation {operation_text}."))?;
    let resource = payload
        .get("resource")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "MTS_HOOK_RESOURCE: resource is required.".to_string())?;
    let decision = load_target_policy(home, target)?.decide(operation, resource);
    let config = load_config(home)?;
    if config.mode != EnforcementMode::Enforce {
        let shadow_decision = match &decision {
            Decision::Allow => "ALLOW",
            Decision::FullBlock(_) => "FULL_BLOCK",
            Decision::PartialBlock(_) => "PARTIAL_BLOCK",
        };
        println!(
            "{}",
            pretty_json(&json!({
                "decision": "ALLOW",
                "observed_decision": shadow_decision,
                "enforcement_mode": config.mode.as_str(),
                "message": if config.mode == EnforcementMode::Warn {
                    "MTS observed a policy match; WARN mode allows the original operation."
                } else {
                    "MTS observed a policy match; SHADOW mode allows the original operation."
                },
                "original_executed": true
            }))?
        );
        return Ok(());
    }
    if !matches!(&decision, Decision::Allow) {
        let session = payload
            .get("session_id")
            .and_then(|value| value.as_str())
            .unwrap_or("default");
        let key = format!(
            "{}|{}|{}|{}",
            target,
            session,
            operation.as_str(),
            mts_core::normalize_resource(resource, cfg!(windows))
        );
        let (state, attempts) = store.record_retry(&key).map_err(store_error)?;
        if state == "CIRCUIT_OPEN" || attempts >= 3 {
            println!(
                "{}",
                pretty_json(&json!({
                    "decision": "FULL_BLOCK",
                    "reason_code": "MTS_RETRY_CIRCUIT_OPEN",
                    "message": "Equivalent retries exhausted the session budget.",
                    "retry_circuit": "OPEN",
                    "intent_id": key,
                    "original_executed": false
                }))?
            );
            return Ok(());
        }
    }
    println!(
        "{}",
        pretty_json(&decision_json(
            &decision,
            target,
            operation,
            resource,
            config.mode,
            None
        ))?
    );
    Ok(())
}

#[derive(Debug)]
struct NativeHookEvaluation {
    decision: Decision,
    operation: Operation,
    resource: String,
    query: Option<String>,
    line_range: Option<(u64, u64)>,
    session: String,
}

fn native_hook_dispatch(
    home: &Path,
    store: &Store,
    target: &str,
    provider: NativeHookProvider,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let policy = load_target_policy(home, target)?;
    let evaluation = match evaluate_native_hook(provider, payload, &policy) {
        Ok(evaluation) => evaluation,
        Err(message) => {
            println!(
                "{}",
                pretty_json(&provider_error_output(provider, &message))?
            );
            return Ok(());
        }
    };
    let config = load_config(home)?;
    if config.mode == EnforcementMode::Enforce
        && let Decision::PartialBlock(partial) = &evaluation.decision
    {
        let key = native_intent_key(target, &evaluation);
        let (state, attempts) = store.record_retry(&key).map_err(store_error)?;
        if state == "CIRCUIT_OPEN" || attempts >= 3 {
            println!(
                "{}",
                pretty_json(&provider_error_output(
                    provider,
                    "MTS_RETRY_CIRCUIT_OPEN: equivalent retries exhausted the session budget."
                ))?
            );
            return Ok(());
        }
        let output = match substitute_output(
            &evaluation.resource,
            partial,
            evaluation.query.as_deref(),
            None,
        ) {
            Ok(output) => output,
            Err(error) => {
                println!(
                    "{}",
                    pretty_json(&provider_error_output(
                        provider,
                        &format!("MTS_PARTIAL_FAILED_CLOSED: {error}")
                    ))?
                );
                return Ok(());
            }
        };
        let context = partial_context(partial, &output);
        let protected = fs::metadata(&evaluation.resource)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map_or(0, |metadata| metadata.len());
        store
            .record_event(
                target,
                evaluation.operation.as_str(),
                "PARTIAL_BLOCK",
                &evaluation.resource,
                protected,
                protected,
                context.len() as u64,
                if attempts > 1 {
                    context.len() as u64
                } else {
                    0
                },
            )
            .map_err(store_error)?;
        println!(
            "{}",
            pretty_json(&provider_partial_output(provider, partial, &context))?
        );
        return Ok(());
    }
    if config.mode == EnforcementMode::Enforce && !matches!(&evaluation.decision, Decision::Allow) {
        let key = native_intent_key(target, &evaluation);
        let (state, attempts) = store.record_retry(&key).map_err(store_error)?;
        if state == "CIRCUIT_OPEN" || attempts >= 3 {
            println!(
                "{}",
                pretty_json(&provider_error_output(
                    provider,
                    "MTS_RETRY_CIRCUIT_OPEN: equivalent retries exhausted the session budget."
                ))?
            );
            return Ok(());
        }
        if matches!(&evaluation.decision, Decision::FullBlock(_)) {
            let protected = fs::metadata(&evaluation.resource)
                .ok()
                .filter(|metadata| metadata.is_file())
                .map_or(0, |metadata| metadata.len());
            let avoided = if matches!(evaluation.operation, Operation::Read | Operation::Search) {
                protected
            } else {
                0
            };
            store
                .record_event(
                    target,
                    evaluation.operation.as_str(),
                    "FULL_BLOCK",
                    &evaluation.resource,
                    protected,
                    avoided,
                    0,
                    0,
                )
                .map_err(store_error)?;
        }
    }
    println!(
        "{}",
        pretty_json(&provider_decision_output(
            provider,
            &evaluation,
            config.mode,
            target
        ))?
    );
    Ok(())
}

fn native_intent_key(target: &str, evaluation: &NativeHookEvaluation) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        target,
        evaluation.session,
        evaluation.operation.as_str(),
        mts_core::normalize_resource(&evaluation.resource, cfg!(windows)),
        evaluation.query.as_deref().unwrap_or(""),
        evaluation
            .line_range
            .map(|(start, end)| format!("{start}-{end}"))
            .unwrap_or_default()
    )
}

fn evaluate_native_hook(
    provider: NativeHookProvider,
    payload: &serde_json::Value,
    policy: &PolicySet,
) -> Result<NativeHookEvaluation, String> {
    let (tool_name, session, args) = if provider == NativeHookProvider::Antigravity {
        (
            payload
                .pointer("/toolCall/name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "MTS_HOOK_TOOL: toolCall.name is required.".to_string())?,
            payload
                .get("conversationId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("default")
                .to_string(),
            payload
                .pointer("/toolCall/args")
                .ok_or_else(|| "MTS_HOOK_INPUT: toolCall.args is required.".to_string())?,
        )
    } else {
        if payload
            .get("hook_event_name")
            .and_then(serde_json::Value::as_str)
            != Some("PreToolUse")
        {
            return Err("MTS_HOOK_EVENT: expected PreToolUse.".into());
        }
        (
            payload
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "MTS_HOOK_TOOL: tool_name is required.".to_string())?,
            payload
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("default")
                .to_string(),
            payload
                .get("tool_input")
                .ok_or_else(|| "MTS_HOOK_INPUT: tool_input is required.".to_string())?,
        )
    };
    let candidates = hook_candidates(provider, tool_name, args)?;
    let mut selected = (
        Operation::Mcp,
        tool_name.to_string(),
        None,
        None,
        Decision::Allow,
    );
    for (operation, resource, query, line_range) in candidates {
        let decision = policy.decide(operation, &resource);
        if decision_rank(&decision) > decision_rank(&selected.4) {
            selected = (operation, resource, query, line_range, decision);
        }
    }
    let (operation, resource, query, line_range, decision) = selected;
    Ok(NativeHookEvaluation {
        decision,
        operation,
        resource,
        query,
        line_range,
        session,
    })
}

type HookCandidate = (Operation, String, Option<String>, Option<(u64, u64)>);

fn input_string<'a>(input: &'a serde_json::Value, fields: &[&str]) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| input.get(*field).and_then(serde_json::Value::as_str))
}

fn required_input(input: &serde_json::Value, fields: &[&str]) -> Result<String, String> {
    input_string(input, fields)
        .map(str::to_string)
        .ok_or_else(|| format!("MTS_HOOK_INPUT: {} is required.", fields.join(" or ")))
}

fn shell_candidates(command: &str) -> Result<Vec<HookCandidate>, String> {
    let family = if cfg!(windows) {
        ShellFamily::PowerShell
    } else {
        ShellFamily::Unix
    };
    Ok(extract_shell_intents(command, family)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|intent| {
            (
                intent.operation,
                intent.resource,
                intent.search_query,
                intent.line_range,
            )
        })
        .collect())
}

fn search_candidate(
    input: &serde_json::Value,
    path_fields: &[&str],
    query_fields: &[&str],
) -> HookCandidate {
    let path = input_string(input, path_fields).unwrap_or(".");
    let query = input_string(input, query_fields);
    let resource = query.filter(|_| path == ".").unwrap_or(path).to_string();
    (Operation::Search, resource, query.map(str::to_string), None)
}

fn hook_candidates(
    provider: NativeHookProvider,
    tool_name: &str,
    input: &serde_json::Value,
) -> Result<Vec<HookCandidate>, String> {
    let direct = |operation, fields: &[&str]| {
        required_input(input, fields).map(|resource| vec![(operation, resource, None, None)])
    };
    match (provider, tool_name) {
        (NativeHookProvider::Antigravity, "run_command") => {
            shell_candidates(&required_input(input, &["CommandLine"])?)
        }
        (NativeHookProvider::Antigravity, "view_file") => {
            direct(Operation::Read, &["AbsolutePath"])
        }
        (NativeHookProvider::Antigravity, "write_to_file") => {
            direct(Operation::Write, &["TargetFile"])
        }
        (
            NativeHookProvider::Antigravity,
            "replace_file_content" | "multi_replace_file_content",
        ) => direct(Operation::Edit, &["TargetFile"]),
        (NativeHookProvider::Antigravity, "list_dir") => {
            direct(Operation::Search, &["DirectoryPath"])
        }
        (NativeHookProvider::Antigravity, "find_by_name") => Ok(vec![search_candidate(
            input,
            &["SearchDirectory"],
            &["Pattern"],
        )]),
        (NativeHookProvider::Antigravity, "grep_search") => {
            Ok(vec![search_candidate(input, &["SearchPath"], &["Query"])])
        }
        (_, "Bash") => shell_candidates(&required_input(input, &["command"])?),
        (NativeHookProvider::Codex, "apply_patch") => {
            apply_patch_resources(&required_input(input, &["command"])?).map(|items| {
                items
                    .into_iter()
                    .map(|(operation, resource)| (operation, resource, None, None))
                    .collect()
            })
        }
        (_, "Read") => direct(Operation::Read, &["file_path", "path"]),
        (_, "Write") => direct(Operation::Write, &["file_path", "path"]),
        (_, "Edit" | "MultiEdit") => direct(Operation::Edit, &["file_path", "path"]),
        (_, "Glob") => Ok(vec![search_candidate(input, &["path"], &["pattern"])]),
        (_, "Grep") => Ok(vec![search_candidate(
            input,
            &["path", "glob"],
            &["pattern"],
        )]),
        _ => Ok(Vec::new()),
    }
}

fn apply_patch_resources(command: &str) -> Result<Vec<(Operation, String)>, String> {
    let resources = command
        .lines()
        .filter_map(|line| {
            [
                ("*** Add File: ", Operation::Write),
                ("*** Update File: ", Operation::Edit),
                ("*** Delete File: ", Operation::Edit),
                ("*** Move to: ", Operation::Write),
            ]
            .into_iter()
            .find_map(|(prefix, operation)| {
                line.strip_prefix(prefix)
                    .map(|path| (operation, path.trim().to_string()))
            })
        })
        .filter(|(_, path)| !path.is_empty())
        .collect::<Vec<_>>();
    if resources.is_empty() {
        Err("MTS_CODEX_PATCH: apply_patch contains no file operation.".into())
    } else {
        Ok(resources)
    }
}

fn standard_decision_output(
    evaluation: &NativeHookEvaluation,
    mode: EnforcementMode,
    _target: &str,
) -> serde_json::Value {
    let observed = match &evaluation.decision {
        Decision::Allow => "ALLOW",
        Decision::FullBlock(_) => "FULL_BLOCK",
        Decision::PartialBlock(_) => "PARTIAL_BLOCK",
    };
    if mode != EnforcementMode::Enforce {
        return json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "additionalContext": format!(
                    "MTS {} mode observed {observed}; the original tool call is allowed.",
                    mode.as_str()
                )
            }
        });
    }
    match &evaluation.decision {
        Decision::Allow => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow"
            }
        }),
        Decision::FullBlock(block) => standard_error_output(&format!(
            "{}: {}. Do not retry or work around this block. Use relevant source or metadata instead (rule {}).",
            block.reason_code, block.reason, block.rule_id
        )),
        Decision::PartialBlock(block) => standard_error_output(&format!(
            "{}: {} Use the {} bounded alternative instead (rule {}).",
            block.reason_code,
            block.reason,
            block.substitute.mode.as_str(),
            block.rule_id
        )),
    }
}

fn standard_partial_output(
    partial: &mts_core::PartialBlockDecision,
    context: &str,
) -> serde_json::Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": format!(
                "MTS blocked the original command and returned a {} bounded substitute: {}. Do not retry or work around this block (rule {}).",
                partial.substitute.mode.as_str(), partial.reason, partial.rule_id
            ),
            "additionalContext": context
        }
    })
}

fn partial_context(partial: &mts_core::PartialBlockDecision, output: &str) -> String {
    format!(
        "MTS guidance: {}. This policy avoids loading more context than necessary. Do not retry or work around this block; use the bounded result below.\n\n{}",
        partial.reason, output
    )
}

fn standard_error_output(reason: &str) -> serde_json::Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason
        }
    })
}

fn provider_decision_output(
    provider: NativeHookProvider,
    evaluation: &NativeHookEvaluation,
    mode: EnforcementMode,
    target: &str,
) -> serde_json::Value {
    if provider == NativeHookProvider::Antigravity {
        antigravity_decision_output(evaluation, mode)
    } else if provider == NativeHookProvider::Claude {
        claude_decision_output(evaluation, mode, target)
    } else {
        standard_decision_output(evaluation, mode, target)
    }
}

fn claude_decision_output(
    evaluation: &NativeHookEvaluation,
    mode: EnforcementMode,
    target: &str,
) -> serde_json::Value {
    if mode == EnforcementMode::Enforce && matches!(evaluation.decision, Decision::Allow) {
        return json!({});
    }
    let mut output = standard_decision_output(evaluation, mode, target);
    if let Some(fields) = output
        .get_mut("hookSpecificOutput")
        .and_then(serde_json::Value::as_object_mut)
        && fields.get("permissionDecision") == Some(&json!("allow"))
    {
        fields.remove("permissionDecision");
    }
    output
}

fn provider_partial_output(
    provider: NativeHookProvider,
    partial: &mts_core::PartialBlockDecision,
    context: &str,
) -> serde_json::Value {
    if provider == NativeHookProvider::Antigravity {
        json!({
            "decision": "deny",
            "reason": format!(
                "MTS blocked the original operation and returned a {} bounded substitute. Do not retry or work around this block (rule {}).\n\n{}",
                partial.substitute.mode.as_str(), partial.rule_id, context
            )
        })
    } else {
        standard_partial_output(partial, context)
    }
}

fn provider_error_output(provider: NativeHookProvider, reason: &str) -> serde_json::Value {
    if provider == NativeHookProvider::Antigravity {
        json!({ "decision": "deny", "reason": reason })
    } else {
        standard_error_output(reason)
    }
}

fn antigravity_decision_output(
    evaluation: &NativeHookEvaluation,
    mode: EnforcementMode,
) -> serde_json::Value {
    let observed = match &evaluation.decision {
        Decision::Allow => "ALLOW",
        Decision::FullBlock(_) => "FULL_BLOCK",
        Decision::PartialBlock(_) => "PARTIAL_BLOCK",
    };
    if mode != EnforcementMode::Enforce {
        return json!({
            "decision": "ask",
            "reason": format!(
                "MTS {} mode observed {observed}; normal Antigravity permission handling still applies.",
                mode.as_str()
            )
        });
    }
    match &evaluation.decision {
        Decision::Allow => json!({
            "decision": "ask",
            "reason": "MTS allows this operation; normal Antigravity permission handling still applies."
        }),
        Decision::FullBlock(block) => json!({
            "decision": "deny",
            "reason": format!(
                "{}: {}. Do not retry or work around this block. Use relevant source or metadata instead (rule {}).",
                block.reason_code, block.reason, block.rule_id
            )
        }),
        Decision::PartialBlock(block) => json!({
            "decision": "deny",
            "reason": format!(
                "{}: {} Use the {} bounded alternative instead (rule {}).",
                block.reason_code,
                block.reason,
                block.substitute.mode.as_str(),
                block.rule_id
            )
        }),
    }
}

fn passthrough(args: PassthroughArgs) -> Result<(), String> {
    let separator = args
        .arguments
        .iter()
        .position(|argument| argument == "--")
        .map_or(0, |index| index + 1);
    let command = args
        .arguments
        .get(separator)
        .ok_or_else(|| "MTS_WRAPPER_COMMAND: command is required after --.".to_string())?;
    let status = ProcessCommand::new(command)
        .args(&args.arguments[separator + 1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("MTS_WRAPPER_START: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("MTS_WRAPPER_EXIT: child exited with {status}."))
    }
}

fn worker(home: &Path, _args: PassthroughArgs) -> Result<(), String> {
    println!("MTS worker ready. Home: {}", home.display());
    Ok(())
}

fn self_manage(home: &Path, args: SelfArgs) -> Result<(), String> {
    match args.command {
        SelfCommand::Rollback => {
            let backups = home.join("backups");
            let latest = fs::read_dir(&backups)
                .map_err(io_error("MTS_ROLLBACK_READ"))?
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .max_by_key(|entry| entry.file_name());
            let Some(latest) = latest else {
                return Err("MTS_ROLLBACK_EMPTY: no setup backup is available.".into());
            };
            println!(
                "Latest backup: {}\nUse mts uninstall before restoring user configuration from this directory.",
                latest.path().display()
            );
            Ok(())
        }
    }
}

fn preset(name: &str) -> Result<(&'static str, &'static str), String> {
    match name {
        "minimal" => Ok((MINIMAL_FULL, MINIMAL_PARTIAL)),
        "balanced" => Ok((BALANCED_FULL, BALANCED_PARTIAL)),
        "strict" => Ok((STRICT_FULL, STRICT_PARTIAL)),
        _ => Err("MTS_PROFILE_UNKNOWN: choose minimal, balanced, or strict.".into()),
    }
}

fn detect_installed() -> Vec<&'static Target> {
    let commands = registry()
        .iter()
        .flat_map(|target| target.detection.commands.iter().copied())
        .filter(|command| command_exists(command))
        .collect::<Vec<_>>();
    detect_targets(&DetectionEvidence {
        commands: &commands,
        markers: &[],
    })
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| {
            if cfg!(windows) {
                ["exe", "cmd", "bat"]
                    .iter()
                    .any(|extension| directory.join(format!("{command}.{extension}")).is_file())
            } else {
                directory.join(command).is_file()
            }
        })
    })
}

fn resolve_target(id: &str) -> Result<&'static Target, String> {
    target_by_id(id).ok_or_else(|| format!("MTS_TARGET_UNKNOWN: {id} is not registered."))
}

fn capability_json(target: &Target) -> serde_json::Value {
    json!({
        "maximum": target.capability.label(),
        "current": "UNVERIFIED",
        "default_mode": "SHADOW",
        "families": target.families.iter().map(|family| family.as_str()).collect::<Vec<_>>(),
        "mcp_is_enforcement_boundary": false
    })
}

struct HookPlan {
    provider: NativeHookProvider,
    path: PathBuf,
    original: Option<Vec<u8>>,
    contents: Vec<u8>,
    created_by_mts: bool,
}

impl HookPlan {
    fn load(
        home: &Path,
        provider: NativeHookProvider,
        root_override: Option<&Path>,
    ) -> Result<Self, String> {
        let path = hook_config_path(provider, root_override);
        let original = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("MTS_HOOKS_READ: {error}")),
        };
        let contents = install_mts_hook(provider, original.as_deref())?;
        let created_by_mts = original.is_none() || previous_hook_was_created(home, provider, &path);
        Ok(Self {
            provider,
            path,
            original,
            contents,
            created_by_mts,
        })
    }
}

fn previous_hook_was_created(home: &Path, provider: NativeHookProvider, path: &Path) -> bool {
    fs::read_to_string(
        home.join("harnesses")
            .join(provider.target_id())
            .join("install-manifest.json"),
    )
    .ok()
    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    .and_then(|manifest| manifest.get(provider.manifest_key()).cloned())
    .is_some_and(|metadata| {
        metadata
            .get("created_by_mts")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            && metadata.get("path").and_then(serde_json::Value::as_str)
                == Some(path.to_string_lossy().as_ref())
    })
}

fn user_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn hook_config_path(provider: NativeHookProvider, root_override: Option<&Path>) -> PathBuf {
    if let Some(root) = root_override {
        return match provider {
            NativeHookProvider::Codex => root.join("hooks.json"),
            NativeHookProvider::Claude => root.join("claude/settings.json"),
            NativeHookProvider::Antigravity => root.join("antigravity/hooks.json"),
        };
    }
    match provider {
        NativeHookProvider::Codex => env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| user_home().map(|path| path.join(".codex")))
            .unwrap_or_else(|| PathBuf::from(".codex"))
            .join("hooks.json"),
        NativeHookProvider::Claude => env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| user_home().map(|path| path.join(".claude")))
            .unwrap_or_else(|| PathBuf::from(".claude"))
            .join("settings.json"),
        NativeHookProvider::Antigravity => user_home()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".gemini/config/hooks.json"),
    }
}

fn install_mts_hook(
    provider: NativeHookProvider,
    original: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let mut document = match original {
        Some(bytes) => {
            serde_json::from_slice(bytes).map_err(|error| format!("MTS_HOOKS_PARSE: {error}"))?
        }
        None => json!({}),
    };
    if provider == NativeHookProvider::Antigravity {
        install_mts_antigravity_hook(&mut document)?;
    } else {
        remove_mts_standard_handlers(&mut document, provider)?;
        let mut handler = json!({
            "type": "command",
            "command": provider.command(),
            "timeout": 30,
            "statusMessage": MTS_CODEX_HOOK_STATUS
        });
        if provider == NativeHookProvider::Codex {
            handler["commandWindows"] = json!(provider.command());
        }
        pre_tool_groups_mut(&mut document)?.push(json!({
            "matcher": if provider == NativeHookProvider::Codex {
                "^(Bash|apply_patch|Read|Write|Edit|Glob|Grep)$"
            } else {
                "^(Bash|Read|Write|Edit|MultiEdit|Glob|Grep)$"
            },
            "hooks": [handler]
        }));
    }
    Ok(pretty_json(&document)?.into_bytes())
}

fn pre_tool_groups_mut(
    document: &mut serde_json::Value,
) -> Result<&mut Vec<serde_json::Value>, String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "MTS_HOOKS_SCHEMA: root must be an object.".to_string())?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| "MTS_HOOKS_SCHEMA: hooks must be an object.".to_string())?;
    hooks
        .entry("PreToolUse")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| "MTS_HOOKS_SCHEMA: PreToolUse must be an array.".to_string())
}

fn is_mts_standard_handler(handler: &serde_json::Value, provider: NativeHookProvider) -> bool {
    handler
        .get("statusMessage")
        .and_then(|value| value.as_str())
        == Some(MTS_CODEX_HOOK_STATUS)
        && handler.get("command").and_then(|value| value.as_str()) == Some(provider.command())
}

fn remove_mts_standard_handlers(
    document: &mut serde_json::Value,
    provider: NativeHookProvider,
) -> Result<bool, String> {
    let Some(root) = document.as_object_mut() else {
        return Err("MTS_HOOKS_SCHEMA: root must be an object.".into());
    };
    let Some(hooks_value) = root.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks_value
        .as_object_mut()
        .ok_or_else(|| "MTS_HOOKS_SCHEMA: hooks must be an object.".to_string())?;
    let Some(groups_value) = hooks.get_mut("PreToolUse") else {
        return Ok(false);
    };
    let groups = groups_value
        .as_array_mut()
        .ok_or_else(|| "MTS_HOOKS_SCHEMA: PreToolUse must be an array.".to_string())?;
    let mut removed = false;
    let mut index = 0;
    while index < groups.len() {
        let (changed, empty) = groups[index]
            .get_mut("hooks")
            .and_then(serde_json::Value::as_array_mut)
            .map_or((false, false), |handlers| {
                let before = handlers.len();
                handlers.retain(|handler| !is_mts_standard_handler(handler, provider));
                (handlers.len() != before, handlers.is_empty())
            });
        removed |= changed;
        if changed && empty {
            groups.remove(index);
        } else {
            index += 1;
        }
    }
    if groups.is_empty() {
        hooks.remove("PreToolUse");
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    Ok(removed)
}

fn antigravity_hook_is_mts(value: &serde_json::Value) -> bool {
    value
        .pointer("/PreToolUse/0/hooks/0/command")
        .and_then(serde_json::Value::as_str)
        == Some(MTS_ANTIGRAVITY_HOOK_COMMAND)
}

fn install_mts_antigravity_hook(document: &mut serde_json::Value) -> Result<(), String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "MTS_HOOKS_SCHEMA: root must be an object.".to_string())?;
    if root
        .get(MTS_ANTIGRAVITY_HOOK_NAME)
        .is_some_and(|value| !antigravity_hook_is_mts(value))
    {
        return Err(format!(
            "MTS_HOOKS_CONFLICT: {MTS_ANTIGRAVITY_HOOK_NAME} is already owned by another Antigravity hook."
        ));
    }
    root.insert(
        MTS_ANTIGRAVITY_HOOK_NAME.into(),
        json!({
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": MTS_ANTIGRAVITY_HOOK_COMMAND,
                    "timeout": 30
                }]
            }]
        }),
    );
    Ok(())
}

fn remove_mts_antigravity_hook(document: &mut serde_json::Value) -> Result<bool, String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "MTS_HOOKS_SCHEMA: root must be an object.".to_string())?;
    if root
        .get(MTS_ANTIGRAVITY_HOOK_NAME)
        .is_some_and(antigravity_hook_is_mts)
    {
        root.remove(MTS_ANTIGRAVITY_HOOK_NAME);
        Ok(true)
    } else {
        Ok(false)
    }
}

fn restore_optional_file(path: &Path, original: Option<&[u8]>) -> Result<(), String> {
    match original {
        Some(bytes) => atomic_write(path, bytes).map_err(io_error("MTS_SETUP_ROLLBACK")),
        None if path.exists() => fs::remove_file(path).map_err(io_error("MTS_SETUP_ROLLBACK")),
        None => Ok(()),
    }
}

fn remove_installed_hook(
    provider: NativeHookProvider,
    manifest: &serde_json::Value,
) -> Result<(), String> {
    let Some(metadata) = manifest.get(provider.manifest_key()) else {
        return Ok(());
    };
    let path = metadata
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "MTS_HOOKS_MANIFEST: path is required.".to_string())?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("MTS_HOOKS_READ: {error}")),
    };
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("MTS_HOOKS_PARSE: {error}"))?;
    let removed = if provider == NativeHookProvider::Antigravity {
        remove_mts_antigravity_hook(&mut document)?
    } else {
        remove_mts_standard_handlers(&mut document, provider)?
    };
    if !removed {
        return Ok(());
    }
    let created_by_mts = metadata
        .get("created_by_mts")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if created_by_mts && document.as_object().is_some_and(serde_json::Map::is_empty) {
        fs::remove_file(path).map_err(io_error("MTS_HOOKS_REMOVE"))
    } else {
        let contents = pretty_json(&document)?;
        atomic_write(&path, contents.as_bytes()).map_err(io_error("MTS_HOOKS_WRITE"))
    }
}

fn update_for(path: PathBuf, contents: Vec<u8>) -> Result<FileUpdate, String> {
    let update = FileUpdate::new(&path, contents);
    match fs::read(path) {
        Ok(original) => Ok(update.expecting_hash(content_hash(&original))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(update),
        Err(error) => Err(format!("MTS_POLICY_READ: {error}")),
    }
}

fn commit_policy_updates(mut updates: Vec<FileUpdate>) -> Result<(), String> {
    let mut manifest_changes = std::collections::BTreeMap::<PathBuf, Vec<(String, String)>>::new();
    for update in &updates {
        let Some(name) = update.path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !matches!(name, "block-full.txt" | "block-partial.txt") {
            continue;
        }
        let Some(directory) = update.path.parent() else {
            continue;
        };
        let manifest = directory.join("install-manifest.json");
        if manifest.is_file() {
            manifest_changes
                .entry(manifest)
                .or_default()
                .push((name.to_string(), content_hash(&update.contents)));
        }
    }
    for (path, changes) in manifest_changes {
        let mut manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&path).map_err(io_error("MTS_MANIFEST_READ"))?,
        )
        .map_err(|error| format!("MTS_MANIFEST_PARSE: {error}"))?;
        let hashes = manifest
            .get_mut("policy_hashes")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| {
                format!(
                    "MTS_MANIFEST_SCHEMA: {} has no policy_hashes object.",
                    path.display()
                )
            })?;
        for (name, hash) in changes {
            hashes.insert(name, serde_json::Value::String(hash));
        }
        updates.push(update_for(path, pretty_json(&manifest)?.into_bytes())?);
    }
    FanoutTransaction::commit(updates, validate_candidate)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn validate_candidate(path: &Path, contents: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(contents).map_err(|_| "Candidate is not UTF-8.".to_string())?;
    match path.file_name().and_then(|name| name.to_str()) {
        Some("block-full.txt") => CompiledPolicy::parse_full(text)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("block-partial.txt") => CompiledPolicy::parse_partial(text)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some(name) if name.ends_with(".json") => serde_json::from_str::<serde_json::Value>(text)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some(name) if name.ends_with(".toml") => toml::from_str::<toml::Value>(text)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        _ => Ok(()),
    }
}

fn validate_policy_files(directory: &Path) -> Result<(), String> {
    validate_one(&directory.join("block-full.txt"))?;
    validate_one(&directory.join("block-partial.txt"))
}

fn validate_one(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(io_error("MTS_POLICY_READ"))?;
    validate_text(path, &text)
}

fn validate_text(path: &Path, text: &str) -> Result<(), String> {
    validate_candidate(path, text.as_bytes())
}

fn load_target_policy(home: &Path, target: &str) -> Result<PolicySet, String> {
    let directory = home.join("harnesses").join(target);
    if !directory.is_dir() && target_by_id(target).is_none() {
        return Err(format!("MTS_TARGET_UNKNOWN: {target} is not registered."));
    }
    let (full, partial) = if directory.is_dir() {
        (
            load_compiled_policy(
                home,
                target,
                &directory.join("block-full.txt"),
                PolicyKind::Full,
            )?,
            load_compiled_policy(
                home,
                target,
                &directory.join("block-partial.txt"),
                PolicyKind::Partial,
            )?,
        )
    } else {
        (
            CompiledPolicy::parse_full(BALANCED_FULL).map_err(|error| error.to_string())?,
            CompiledPolicy::parse_partial(BALANCED_PARTIAL).map_err(|error| error.to_string())?,
        )
    };
    let mut set = PolicySet::new();
    set.add(full, RuleScope::Harness);
    set.add(partial, RuleScope::Harness);
    if let Some(project) = nearest_project(env::current_dir().map_err(io_error("MTS_CWD"))?) {
        let project_config = fs::read_to_string(project.join("project.toml")).unwrap_or_default();
        if project_config.contains("mode = \"isolated\"") {
            set = PolicySet::new();
        }
        if !project_config.contains("mode = \"global\"") {
            let cache_key = format!(
                "project-{}",
                content_hash(project.to_string_lossy().as_bytes()).replace("fnv1a64:", "")
            );
            set.add(
                load_compiled_policy(
                    home,
                    &cache_key,
                    &project.join("block-full.txt"),
                    PolicyKind::Full,
                )?,
                RuleScope::Project,
            );
            set.add(
                load_compiled_policy(
                    home,
                    &cache_key,
                    &project.join("block-partial.txt"),
                    PolicyKind::Partial,
                )?,
                RuleScope::Project,
            );
        }
    }
    Ok(set)
}

fn load_compiled_policy(
    home: &Path,
    cache_key: &str,
    path: &Path,
    kind: PolicyKind,
) -> Result<CompiledPolicy, String> {
    let text = fs::read_to_string(path).map_err(io_error("MTS_POLICY_READ"))?;
    let parse = |source: &str| match kind {
        PolicyKind::Full => CompiledPolicy::parse_full(source),
        PolicyKind::Partial => CompiledPolicy::parse_partial(source),
    };
    let cache = home
        .join("state/policy-cache")
        .join(cache_key)
        .join(path.file_name().unwrap_or_default());
    match parse(&text) {
        Ok(compiled) => {
            atomic_write(&cache, text.as_bytes()).map_err(io_error("MTS_POLICY_CACHE_WRITE"))?;
            Ok(compiled)
        }
        Err(current_error) => {
            let cached = fs::read_to_string(&cache).map_err(|_| {
                format!("{} No last-known-valid policy is available.", current_error)
            })?;
            eprintln!(
                "MTS_POLICY_LAST_VALID: {} is invalid; using the cached valid policy.",
                path.display()
            );
            parse(&cached).map_err(|error| error.to_string())
        }
    }
}

fn run_substitute(
    resource: &str,
    partial: &mts_core::PartialBlockDecision,
    query: Option<&str>,
    diff: Option<&Path>,
) -> Result<u64, String> {
    let output = substitute_output(resource, partial, query, diff)?;
    print!("{output}");
    Ok(output.len() as u64)
}

fn substitute_output(
    resource: &str,
    partial: &mts_core::PartialBlockDecision,
    query: Option<&str>,
    diff: Option<&Path>,
) -> Result<String, String> {
    let output = match partial.substitute.mode {
        mts_core::ReplacementMode::Limit => {
            let mut bounds = ReadBounds::default();
            if let Some(value) = partial.substitute.bounds.get("max_lines") {
                bounds.max_lines = value
                    .parse()
                    .map_err(|_| "MTS_PARTIAL_BOUNDS: invalid max_lines.".to_string())?;
            }
            if let Some(value) = partial.substitute.bounds.get("max_bytes") {
                bounds.max_bytes = value
                    .parse()
                    .map_err(|_| "MTS_PARTIAL_BOUNDS: invalid max_bytes.".to_string())?;
            }
            let result =
                bounded_read(Path::new(resource), bounds).map_err(|error| error.to_string())?;
            format!(
                "{}\n[MTS bounded result: returned_bytes={}, omitted_bytes={}, truncated={}]\n",
                result.text, result.returned_bytes, result.omitted_bytes, result.truncated
            )
        }
        mts_core::ReplacementMode::MetadataOnly => {
            let result =
                bounded_metadata(Path::new(resource), 500).map_err(|error| error.to_string())?;
            format!(
                "[MTS metadata: kind={:?}, bytes={}, files={}, directories={}, truncated={}]\n",
                result.kind,
                result.bytes,
                result.file_count,
                result.directory_count,
                result.truncated
            )
        }
        mts_core::ReplacementMode::SearchOnly | mts_core::ReplacementMode::SymbolOnly => {
            if let Some(query) = query {
                let resource = Path::new(resource);
                let root = if resource.is_dir() {
                    resource
                } else {
                    resource.parent().unwrap_or_else(|| Path::new("."))
                };
                let result = bounded_search(root, query, SearchBounds::default())
                    .map_err(|error| error.to_string())?;
                let mut output = String::new();
                for found in &result.matches {
                    output.push_str(&format!("{}:{}\n", found.path.display(), found.line_number));
                    for context in &found.context {
                        output.push_str(&format!("  {}: {}\n", context.line_number, context.text));
                    }
                }
                output.push_str(&format!(
                    "[MTS bounded search: matches={}, files_scanned={}, bytes_scanned={}, truncated={}]\n",
                    result.matches.len(),
                    result.files_scanned,
                    result.bytes_scanned,
                    result.truncated
                ));
                output
            } else {
                format!(
                    "MTS_PARTIAL_DISCLOSURE: the original operation was not executed; provide a literal search query for {}.\n",
                    partial.substitute.mode.as_str()
                )
            }
        }
        mts_core::ReplacementMode::ErrorsOnly => {
            let bytes = fs::read(resource).map_err(io_error("MTS_PARTIAL_READ"))?;
            if bytes.len() as u64 > mts_core::MAX_REPLACEMENT_SCAN_BYTES {
                return Err(
                    "MTS_PARTIAL_INPUT_TOO_LARGE: error extraction input exceeds the 16 MiB safety limit."
                        .into(),
                );
            }
            let input = String::from_utf8(bytes)
                .map_err(|_| "MTS_PARTIAL_BINARY_RESOURCE: log is not UTF-8 text.".to_string())?;
            let result = extract_error_regions(&input, ErrorBounds::default())
                .map_err(|error| error.to_string())?;
            format!(
                "{}[MTS error regions: returned_bytes={}, omitted_bytes={}, truncated={}]\n",
                result.text, result.returned_bytes, result.omitted_bytes, result.truncated
            )
        }
        mts_core::ReplacementMode::PatchOnly => {
            let Some(diff) = diff else {
                return Err("MTS_PATCH_REQUIRED: pass --diff with a persistent patch.".into());
            };
            let bytes = fs::read(diff).map_err(io_error("MTS_PATCH_READ"))?;
            if bytes.len() > 1024 * 1024 {
                return Err("MTS_PATCH_TOO_LARGE: patch exceeds the 1 MiB safety limit.".into());
            }
            let text = String::from_utf8(bytes.clone())
                .map_err(|_| "MTS_PATCH_ENCODING: patch must be UTF-8 text.".to_string())?;
            let changed = text
                .lines()
                .filter(|line| {
                    (line.starts_with('+') && !line.starts_with("+++"))
                        || (line.starts_with('-') && !line.starts_with("---"))
                })
                .count();
            let maximum = partial
                .substitute
                .bounds
                .get("max_changed_lines")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(30);
            if changed > maximum {
                return Err(format!(
                    "MTS_PATCH_LIMIT: patch changes {changed} lines; maximum is {maximum}."
                ));
            }
            format!(
                "MTS patch artifact: {}\nHash: {}\nChanged lines: {}\nThe protected dependency was not edited.\n",
                diff.display(),
                content_hash(&bytes),
                changed
            )
        }
        mts_core::ReplacementMode::Redirect => {
            let target = partial
                .substitute
                .bounds
                .get("target")
                .or_else(|| partial.substitute.bounds.get("source_map"))
                .map(String::as_str)
                .unwrap_or("the configured generator source");
            format!("MTS_REDIRECT: the original operation was not executed; edit {target}.\n")
        }
    };
    Ok(output)
}

fn decision_json(
    decision: &Decision,
    target: &str,
    operation: Operation,
    resource: &str,
    mode: EnforcementMode,
    shell_intents: Option<usize>,
) -> serde_json::Value {
    match decision {
        Decision::Allow => json!({
            "decision": "ALLOW",
            "target": target,
            "operation": operation.as_str(),
            "resource": resource,
            "enforcement_mode": mode.as_str(),
            "shell_intents": shell_intents
        }),
        Decision::FullBlock(block) => json!({
            "decision": "FULL_BLOCK",
            "reason_code": block.reason_code,
            "rule_id": block.rule_id,
            "message": block.reason,
            "pattern": block.matched_pattern,
            "target": target,
            "operation": operation.as_str(),
            "resource": resource,
            "original_executed": false,
            "enforcement_mode": mode.as_str(),
            "shell_intents": shell_intents
        }),
        Decision::PartialBlock(block) => json!({
            "decision": "PARTIAL_BLOCK",
            "reason_code": block.reason_code,
            "rule_id": block.rule_id,
            "message": block.reason,
            "pattern": block.matched_pattern,
            "replacement_mode": block.substitute.mode.as_str(),
            "bounds": block.substitute.bounds,
            "target": target,
            "operation": operation.as_str(),
            "resource": resource,
            "original_executed": false,
            "enforcement_mode": mode.as_str(),
            "shell_intents": shell_intents
        }),
    }
}

fn policy_paths(
    home: &Path,
    store: &Store,
    scope: &Scope,
    kind: Option<PolicyKind>,
) -> Result<Vec<PathBuf>, String> {
    let names: Vec<&str> = match kind {
        Some(PolicyKind::Full) => vec!["block-full.txt"],
        Some(PolicyKind::Partial) => vec!["block-partial.txt"],
        None => vec!["block-full.txt", "block-partial.txt"],
    };
    if let Some(project) = &scope.project {
        return Ok(names
            .iter()
            .map(|name| project.join(".mts").join(name))
            .collect());
    }
    let targets = if let Some(target) = &scope.target {
        vec![target.clone()]
    } else if scope.targets.is_empty() || scope.targets == ["all"] {
        store.installed_targets().map_err(store_error)?
    } else {
        scope.targets.clone()
    };
    if targets.is_empty() {
        if let Some(project) = nearest_project(env::current_dir().map_err(io_error("MTS_CWD"))?) {
            return Ok(names.iter().map(|name| project.join(name)).collect());
        }
        return Err("MTS_POLICY_SCOPE: no installed target or project policy was found.".into());
    }
    Ok(targets
        .into_iter()
        .flat_map(|target| {
            names
                .iter()
                .map(move |name| home.join("harnesses").join(&target).join(name))
        })
        .collect())
}

fn mutate_policy<F>(
    home: &Path,
    store: &Store,
    scope: &Scope,
    kind: PolicyKind,
    mutation: F,
) -> Result<(), String>
where
    F: Fn(&str) -> Result<String, String>,
{
    let paths = policy_paths(home, store, scope, Some(kind))?;
    let mut updates = Vec::new();
    for path in paths {
        let current = fs::read_to_string(&path).map_err(io_error("MTS_POLICY_READ"))?;
        let candidate = mutation(&current)?;
        validate_text(&path, &candidate)?;
        updates.push(update_for(path, candidate.into_bytes())?);
    }
    commit_policy_updates(updates)?;
    println!("Policy transaction committed.");
    Ok(())
}

fn add_rule(text: &str, kind: PolicyKind, rule: &str) -> Result<String, String> {
    let prefix = match kind {
        PolicyKind::Full => "MTS-FULL",
        PolicyKind::Partial => "MTS-PARTIAL",
    };
    let candidate = if rule.trim_start().starts_with('@') {
        rule.trim().to_string()
    } else {
        let hash = content_hash(rule.as_bytes())
            .replace("fnv1a64:", "")
            .to_ascii_uppercase();
        format!("@{prefix}-{hash} {}", rule.trim())
    };
    match kind {
        PolicyKind::Full => CompiledPolicy::parse_full(&candidate),
        PolicyKind::Partial => CompiledPolicy::parse_partial(&candidate),
    }
    .map_err(|error| error.to_string())?;
    if text.trim().is_empty() {
        Ok(format!("{candidate}\n"))
    } else {
        Ok(format!("{}\n{candidate}\n", text.trim_end()))
    }
}

fn replace_rule(text: &str, rule_id: &str, replacement: Option<&str>) -> Result<String, String> {
    let bare_id = rule_id.trim_start_matches('@');
    let needle = format!("@{bare_id} ");
    let mut found = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with(&needle) {
            found = true;
            if let Some(replacement) = replacement {
                lines.push(format!("@{bare_id} {}", replacement.trim()));
            }
        } else {
            lines.push(line.to_string());
        }
    }
    if !found {
        return Err(format!("MTS_RULE_NOT_FOUND: {rule_id} was not found."));
    }
    Ok(format!("{}\n", lines.join("\n").trim_end()))
}

fn format_policy(text: &str) -> String {
    let lines = text.lines().map(str::trim_end).collect::<Vec<_>>();
    format!("{}\n", lines.join("\n").trim())
}

fn backup_owned_files(source: &Path, destination: &Path) -> Result<(), String> {
    for name in [
        "block-full.txt",
        "block-partial.txt",
        "adapter.json",
        "adapter.toml",
        "install-manifest.json",
    ] {
        let path = source.join(name);
        if path.is_file() {
            fs::create_dir_all(destination).map_err(io_error("MTS_BACKUP_DIRECTORY"))?;
            fs::copy(&path, destination.join(name)).map_err(io_error("MTS_BACKUP_WRITE"))?;
        }
    }
    Ok(())
}

fn rollback_setup_files(home: &Path, backup_root: &Path, targets: &[String]) -> Result<(), String> {
    for target in targets {
        let directory = home.join("harnesses").join(target);
        let backup = backup_root.join(target);
        for name in [
            "block-full.txt",
            "block-partial.txt",
            "adapter.json",
            "adapter.toml",
            "install-manifest.json",
        ] {
            let path = directory.join(name);
            let saved = backup.join(name);
            if saved.is_file() {
                fs::copy(saved, path).map_err(io_error("MTS_SETUP_ROLLBACK"))?;
            } else if path.is_file() {
                fs::remove_file(path).map_err(io_error("MTS_SETUP_ROLLBACK"))?;
            }
        }
        if directory.is_dir()
            && fs::read_dir(&directory)
                .map_err(io_error("MTS_SETUP_ROLLBACK"))?
                .next()
                .is_none()
        {
            fs::remove_dir(directory).map_err(io_error("MTS_SETUP_ROLLBACK"))?;
        }
    }
    Ok(())
}

fn restore_config(home: &Path, original: Option<&[u8]>) -> Result<(), String> {
    let path = home.join("config.toml");
    if let Some(original) = original {
        atomic_write(&path, original).map_err(io_error("MTS_SETUP_ROLLBACK"))
    } else if path.exists() {
        fs::remove_file(path).map_err(io_error("MTS_SETUP_ROLLBACK"))
    } else {
        Ok(())
    }
}

fn remove_target(home: &Path, store: &Store, target: &str) -> Result<(), String> {
    let directory = home.join("harnesses").join(target);
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.join("install-manifest.json")).unwrap_or_default(),
    )
    .unwrap_or_default();
    if let Some(provider) = NativeHookProvider::for_target(target) {
        remove_installed_hook(provider, &manifest)?;
    }
    let backup = latest_target_backup(home, target);
    for name in [
        "block-full.txt",
        "block-partial.txt",
        "adapter.json",
        "adapter.toml",
        "install-manifest.json",
    ] {
        let path = directory.join(name);
        if !path.is_file() {
            continue;
        }
        if matches!(name, "block-full.txt" | "block-partial.txt")
            && manifest
                .get("policy_hashes")
                .and_then(|hashes| hashes.get(name))
                .and_then(|value| value.as_str())
                .is_some_and(|expected| {
                    fs::read(&path)
                        .map(|bytes| content_hash(&bytes) != expected)
                        .unwrap_or(true)
                })
        {
            println!(
                "MTS_UNINSTALL_PRESERVED: {} changed after installation and was not overwritten.",
                path.display()
            );
            continue;
        }
        let backup_file = backup.as_ref().map(|directory| directory.join(name));
        if let Some(backup_file) = backup_file.filter(|path| path.is_file()) {
            fs::copy(backup_file, &path).map_err(io_error("MTS_UNINSTALL_RESTORE"))?;
        } else {
            fs::remove_file(path).map_err(io_error("MTS_UNINSTALL_REMOVE"))?;
        }
    }
    if directory.is_dir()
        && fs::read_dir(&directory)
            .map_err(io_error("MTS_UNINSTALL_READ"))?
            .next()
            .is_none()
    {
        fs::remove_dir(directory).map_err(io_error("MTS_UNINSTALL_REMOVE"))?;
    }
    store.remove_installation(target).map_err(store_error)?;
    println!("Removed MTS integration for {target}.");
    Ok(())
}

fn latest_target_backup(home: &Path, target: &str) -> Option<PathBuf> {
    fs::read_dir(home.join("backups"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(target))
        .filter(|path| path.is_dir())
        .max()
}

fn verify_target(id: &str) -> Result<(), String> {
    let target = resolve_target(id)?;
    let Some(command) = target
        .detection
        .commands
        .iter()
        .find(|command| command_exists(command))
    else {
        return Err(format!(
            "MTS_VERIFY_NOT_DETECTED: {id} is not installed or not on PATH."
        ));
    };
    let mut child = ProcessCommand::new(command)
        .args(target.detection.version_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("MTS_VERIFY_START: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if child
            .try_wait()
            .map_err(|error| format!("MTS_VERIFY_WAIT: {error}"))?
            .is_some()
        {
            let output = child
                .wait_with_output()
                .map_err(|error| format!("MTS_VERIFY_OUTPUT: {error}"))?;
            let version = String::from_utf8_lossy(&output.stdout);
            println!(
                "Target: {id}\nDetected version: {}\nContract: UNVERIFIED\nMode: SHADOW",
                version.trim()
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child
        .kill()
        .map_err(|error| format!("MTS_VERIFY_KILL: {error}"))?;
    Err("MTS_ADAPTER_TIMEOUT: version probe exceeded 3 seconds.".into())
}

fn drift(home: &Path, store: &Store) -> Result<(), String> {
    for target in store.installed_targets().map_err(store_error)? {
        let directory = home.join("harnesses").join(&target);
        println!(
            "{target}\t{}",
            if target_has_drift(&directory) {
                "DRIFT"
            } else {
                "CLEAN"
            }
        );
    }
    Ok(())
}

fn target_has_drift(directory: &Path) -> bool {
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(directory.join("install-manifest.json")).unwrap_or_default(),
    )
    .unwrap_or_default();
    let expected = manifest.get("policy_hashes").cloned().unwrap_or_default();
    for name in ["block-full.txt", "block-partial.txt"] {
        let actual = fs::read(directory.join(name))
            .map(|bytes| content_hash(&bytes))
            .unwrap_or_default();
        if expected.get(name).and_then(|value| value.as_str()) != Some(&actual) {
            return true;
        }
    }
    false
}

fn sync_policy(home: &Path, store: &Store, from: &str, to: &[String]) -> Result<(), String> {
    let source = home.join("harnesses").join(from);
    let full = fs::read(source.join("block-full.txt")).map_err(io_error("MTS_SYNC_READ"))?;
    let partial = fs::read(source.join("block-partial.txt")).map_err(io_error("MTS_SYNC_READ"))?;
    let targets = if to == ["all"] {
        store.installed_targets().map_err(store_error)?
    } else {
        to.to_vec()
    };
    let mut updates = Vec::new();
    for target in targets.into_iter().filter(|target| target != from) {
        let directory = home.join("harnesses").join(target);
        updates.push(update_for(directory.join("block-full.txt"), full.clone())?);
        updates.push(update_for(
            directory.join("block-partial.txt"),
            partial.clone(),
        )?);
    }
    commit_policy_updates(updates)?;
    println!("Policy synchronized from {from}.");
    Ok(())
}

fn nearest_project(mut path: PathBuf) -> Option<PathBuf> {
    loop {
        let candidate = path.join(".mts");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !path.pop() {
            return None;
        }
    }
}

fn operation(value: OperationArg) -> Operation {
    match value {
        OperationArg::Read => Operation::Read,
        OperationArg::Write => Operation::Write,
        OperationArg::Edit => Operation::Edit,
        OperationArg::Search => Operation::Search,
        OperationArg::Shell => Operation::Shell,
        OperationArg::Execute => Operation::Execute,
        OperationArg::Mcp => Operation::Mcp,
    }
}

fn canonical(text: &str) -> String {
    format!("{}\n", text.trim_end())
}

fn pretty_json(value: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|error| format!("MTS_JSON: {error}"))
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

fn store_error(error: rusqlite::Error) -> String {
    format!("MTS_STORE_ERROR: {error}")
}

fn io_error(code: &'static str) -> impl FnOnce(io::Error) -> String {
    move |error| format!("{code}: {error}")
}

fn confirm(prompt: &str) -> Result<bool, String> {
    use std::io::Write;
    print!("{prompt}");
    io::stdout().flush().map_err(io_error("MTS_PROMPT_WRITE"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(io_error("MTS_PROMPT_READ"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn prompt_replacement() -> Result<String, String> {
    use std::io::Write;
    print!("Replacement rule body: ");
    io::stdout().flush().map_err(io_error("MTS_PROMPT_WRITE"))?;
    let mut replacement = String::new();
    io::stdin()
        .read_line(&mut replacement)
        .map_err(io_error("MTS_PROMPT_READ"))?;
    if replacement.trim().is_empty() {
        Err("MTS_RULE_EMPTY: replacement rule must not be empty.".into())
    } else {
        Ok(replacement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_policy() -> PolicySet {
        let mut policy = PolicySet::new();
        policy.add(
            CompiledPolicy::parse_full(
                "node_modules/** | write,edit | Installed dependencies are immutable",
            )
            .unwrap(),
            RuleScope::Harness,
        );
        policy.add(
            CompiledPolicy::parse_partial(
                "node_modules/** | read,search | limit | max_lines=20,max_bytes=4096 | Bound dependency reads",
            )
            .unwrap(),
            RuleScope::Harness,
        );
        policy
    }

    fn codex_payload(tool_name: &str, command: &str) -> serde_json::Value {
        standard_payload(tool_name, json!({ "command": command }))
    }

    fn standard_payload(tool_name: &str, input: serde_json::Value) -> serde_json::Value {
        json!({
            "session_id": "session-1",
            "turn_id": "turn-1",
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_use_id": "tool-1",
            "tool_input": input
        })
    }

    fn antigravity_payload(tool_name: &str, input: serde_json::Value) -> serde_json::Value {
        json!({
            "conversationId": "conversation-1",
            "toolCall": {
                "name": tool_name,
                "args": input
            }
        })
    }

    fn temporary_home(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("mts-cli-{name}-{}", timestamp()));
        ensure_layout(&root).unwrap();
        root
    }

    #[test]
    fn generated_rule_is_valid_and_addressable() {
        let text = add_rule(
            "",
            PolicyKind::Full,
            "node_modules/** | edit | Dependencies are immutable",
        )
        .unwrap();
        CompiledPolicy::parse_full(&text).unwrap();
        let id = text
            .split_whitespace()
            .next()
            .unwrap()
            .trim_start_matches('@');
        assert_eq!(replace_rule(&text, id, None).unwrap(), "\n");
    }

    #[test]
    fn project_modes_are_restricted() {
        let root = env::temp_dir().join(format!("mts-project-test-{}", timestamp()));
        fs::create_dir(&root).unwrap();
        assert!(init_project(&root, "overlay").is_ok());
        assert!(init_project(&root, "unknown").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn setup_writes_physical_files_store_rows_and_shadow_config() {
        let root = temporary_home("setup");
        let codex_home = root.join("codex-home");
        let store = Store::open(&root).unwrap();
        setup(
            &root,
            &store,
            SetupArgs {
                profile: "balanced".into(),
                targets: vec![
                    "codex-cli".into(),
                    "claude-code-cli".into(),
                    "antigravity-cli".into(),
                ],
                yes: true,
                dry_run: false,
                codex_home: Some(codex_home.clone()),
                custom: None,
            },
        )
        .unwrap();
        for target in ["codex-cli", "claude-code-cli", "antigravity-cli"] {
            let directory = root.join("harnesses").join(target);
            for name in [
                "block-full.txt",
                "block-partial.txt",
                "adapter.json",
                "install-manifest.json",
            ] {
                assert!(directory.join(name).is_file());
            }
            validate_policy_files(&directory).unwrap();
        }
        assert_eq!(store.installed_targets().unwrap().len(), 3);
        assert_eq!(load_config(&root).unwrap().mode, EnforcementMode::Shadow);
        let hooks: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(codex_home.join("hooks.json")).unwrap())
                .unwrap();
        assert_eq!(
            hooks
                .pointer("/hooks/PreToolUse")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        let claude: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(codex_home.join("claude/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            claude.pointer("/hooks/PreToolUse/0/hooks/0/command"),
            Some(&json!(MTS_CLAUDE_HOOK_COMMAND))
        );
        let antigravity: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(codex_home.join("antigravity/hooks.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            antigravity.pointer("/my-token-scrooge/PreToolUse/0/hooks/0/command"),
            Some(&json!(MTS_ANTIGRAVITY_HOOK_COMMAND))
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_and_shell_aliases_share_the_persisted_retry_circuit() {
        let root = temporary_home("retry");
        let store = Store::open(&root).unwrap();
        let direct = || SimulateArgs {
            operation: OperationArg::Read,
            input: "node_modules/pkg/index.js".into(),
            target: "codex-cli".into(),
            session: "same".into(),
            diff: None,
        };
        simulate(&root, &store, direct()).unwrap();
        simulate(
            &root,
            &store,
            SimulateArgs {
                operation: OperationArg::Shell,
                input: if cfg!(windows) {
                    "Get-Content node_modules\\pkg\\index.js".into()
                } else {
                    "cat node_modules/pkg/index.js".into()
                },
                target: "codex-cli".into(),
                session: "same".into(),
                diff: None,
            },
        )
        .unwrap();
        simulate(&root, &store, direct()).unwrap();
        let rows = store.retry_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "CIRCUIT_OPEN");
        assert_eq!(rows[0].2, 3);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_external_edit_uses_last_valid_policy() {
        let root = temporary_home("last-valid");
        let directory = root.join("harnesses/codex-cli");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("block-full.txt"), BALANCED_FULL).unwrap();
        fs::write(directory.join("block-partial.txt"), BALANCED_PARTIAL).unwrap();
        assert!(matches!(
            load_target_policy(&root, "codex-cli")
                .unwrap()
                .decide(Operation::Edit, "node_modules/pkg/index.js"),
            Decision::FullBlock(_)
        ));
        fs::write(directory.join("block-full.txt"), "malformed").unwrap();
        assert!(matches!(
            load_target_policy(&root, "codex-cli")
                .unwrap()
                .decide(Operation::Edit, "node_modules/pkg/index.js"),
            Decision::FullBlock(_)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sanctioned_policy_update_refreshes_drift_hashes() {
        let root = temporary_home("drift");
        let codex_home = root.join("codex-home");
        let store = Store::open(&root).unwrap();
        setup(
            &root,
            &store,
            SetupArgs {
                profile: "balanced".into(),
                targets: vec!["codex-cli".into()],
                yes: true,
                dry_run: false,
                codex_home: Some(codex_home),
                custom: None,
            },
        )
        .unwrap();
        mutate_policy(
            &root,
            &store,
            &Scope {
                target: Some("codex-cli".into()),
                ..Scope::default()
            },
            PolicyKind::Full,
            |text| {
                add_rule(
                    text,
                    PolicyKind::Full,
                    "vendor/** | edit | Vendor is immutable",
                )
            },
        )
        .unwrap();
        assert!(!target_has_drift(&root.join("harnesses/codex-cli")));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dry_run_validates_without_creating_home() {
        let root = env::temp_dir().join(format!("mts-cli-dry-run-{}", timestamp()));
        setup_preview(
            &root,
            &SetupArgs {
                profile: "balanced".into(),
                targets: vec!["codex-cli".into()],
                yes: false,
                dry_run: true,
                codex_home: Some(root.join("codex-home")),
                custom: None,
            },
        )
        .unwrap();
        assert!(!root.exists());
        assert!(
            setup_preview(
                &root,
                &SetupArgs {
                    profile: "balanced".into(),
                    targets: Vec::new(),
                    yes: false,
                    dry_run: true,
                    codex_home: Some(root.join("codex-home")),
                    custom: Some(CustomSetup::Custom {
                        id: "../escape".into(),
                        command: "agent".into(),
                        workspace_arg: None,
                        mode: "wrapper".into(),
                    }),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn codex_hook_install_and_uninstall_preserve_user_handlers() {
        let root = temporary_home("codex-hooks");
        let codex_home = root.join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        let user_hooks = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "^Bash$",
                    "hooks": [{
                        "type": "command",
                        "command": "user-policy",
                        "statusMessage": "User policy"
                    }]
                }]
            }
        });
        fs::write(
            codex_home.join("hooks.json"),
            pretty_json(&user_hooks).unwrap(),
        )
        .unwrap();
        let store = Store::open(&root).unwrap();
        setup(
            &root,
            &store,
            SetupArgs {
                profile: "balanced".into(),
                targets: vec!["codex-cli".into()],
                yes: true,
                dry_run: false,
                codex_home: Some(codex_home.clone()),
                custom: None,
            },
        )
        .unwrap();
        let installed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(codex_home.join("hooks.json")).unwrap())
                .unwrap();
        assert_eq!(
            installed
                .pointer("/hooks/PreToolUse")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        remove_target(&root, &store, "codex-cli").unwrap();
        let restored: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(codex_home.join("hooks.json")).unwrap())
                .unwrap();
        assert_eq!(restored, user_hooks);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn claude_and_antigravity_install_and_uninstall_preserve_user_config() {
        let root = temporary_home("provider-hooks");
        let hook_root = root.join("hook-root");
        let claude_path = hook_root.join("claude/settings.json");
        let antigravity_path = hook_root.join("antigravity/hooks.json");
        fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        fs::create_dir_all(antigravity_path.parent().unwrap()).unwrap();
        let claude_user = json!({ "permissions": { "allow": ["Read(src/**)"] } });
        let antigravity_user = json!({
            "user-hook": {
                "PostToolUse": [{ "type": "command", "command": "user-check" }]
            }
        });
        fs::write(&claude_path, pretty_json(&claude_user).unwrap()).unwrap();
        fs::write(&antigravity_path, pretty_json(&antigravity_user).unwrap()).unwrap();
        let store = Store::open(&root).unwrap();

        setup(
            &root,
            &store,
            SetupArgs {
                profile: "balanced".into(),
                targets: vec!["claude-code-cli".into(), "antigravity-cli".into()],
                yes: true,
                dry_run: false,
                codex_home: Some(hook_root),
                custom: None,
            },
        )
        .unwrap();
        let claude_installed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&claude_path).unwrap()).unwrap();
        let antigravity_installed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&antigravity_path).unwrap()).unwrap();
        assert_eq!(claude_installed["permissions"], claude_user["permissions"]);
        assert_eq!(
            antigravity_installed["user-hook"],
            antigravity_user["user-hook"]
        );

        remove_target(&root, &store, "claude-code-cli").unwrap();
        remove_target(&root, &store, "antigravity-cli").unwrap();
        let claude_restored: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&claude_path).unwrap()).unwrap();
        let antigravity_restored: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&antigravity_path).unwrap()).unwrap();
        assert_eq!(claude_restored, claude_user);
        assert_eq!(antigravity_restored, antigravity_user);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn claude_and_antigravity_tool_contracts_reach_the_shared_policy() {
        let read_command = if cfg!(windows) {
            "Get-Content node_modules/pkg/index.js"
        } else {
            "cat node_modules/pkg/index.js"
        };
        let cases = vec![
            (
                NativeHookProvider::Claude,
                standard_payload("Bash", json!({ "command": read_command })),
                Operation::Read,
                false,
            ),
            (
                NativeHookProvider::Claude,
                standard_payload("Read", json!({ "file_path": "node_modules/pkg/index.js" })),
                Operation::Read,
                false,
            ),
            (
                NativeHookProvider::Claude,
                standard_payload("Write", json!({ "file_path": "node_modules/pkg/index.js" })),
                Operation::Write,
                true,
            ),
            (
                NativeHookProvider::Claude,
                standard_payload("Edit", json!({ "file_path": "node_modules/pkg/index.js" })),
                Operation::Edit,
                true,
            ),
            (
                NativeHookProvider::Claude,
                standard_payload("Glob", json!({ "pattern": "node_modules/**" })),
                Operation::Search,
                false,
            ),
            (
                NativeHookProvider::Claude,
                standard_payload(
                    "Grep",
                    json!({ "path": "node_modules/pkg", "pattern": "needle" }),
                ),
                Operation::Search,
                false,
            ),
            (
                NativeHookProvider::Antigravity,
                antigravity_payload("run_command", json!({ "CommandLine": read_command })),
                Operation::Read,
                false,
            ),
            (
                NativeHookProvider::Antigravity,
                antigravity_payload(
                    "view_file",
                    json!({ "AbsolutePath": "node_modules/pkg/index.js" }),
                ),
                Operation::Read,
                false,
            ),
            (
                NativeHookProvider::Antigravity,
                antigravity_payload(
                    "write_to_file",
                    json!({ "TargetFile": "node_modules/pkg/index.js" }),
                ),
                Operation::Write,
                true,
            ),
            (
                NativeHookProvider::Antigravity,
                antigravity_payload(
                    "replace_file_content",
                    json!({ "TargetFile": "node_modules/pkg/index.js" }),
                ),
                Operation::Edit,
                true,
            ),
            (
                NativeHookProvider::Antigravity,
                antigravity_payload(
                    "multi_replace_file_content",
                    json!({ "TargetFile": "node_modules/pkg/index.js" }),
                ),
                Operation::Edit,
                true,
            ),
            (
                NativeHookProvider::Antigravity,
                antigravity_payload("list_dir", json!({ "DirectoryPath": "node_modules/pkg" })),
                Operation::Search,
                false,
            ),
            (
                NativeHookProvider::Antigravity,
                antigravity_payload(
                    "find_by_name",
                    json!({ "SearchDirectory": ".", "Pattern": "node_modules/**" }),
                ),
                Operation::Search,
                false,
            ),
            (
                NativeHookProvider::Antigravity,
                antigravity_payload(
                    "grep_search",
                    json!({ "SearchPath": "node_modules/pkg", "Query": "needle" }),
                ),
                Operation::Search,
                false,
            ),
        ];
        for (provider, payload, operation, full_block) in cases {
            let evaluation = evaluate_native_hook(provider, &payload, &codex_policy()).unwrap();
            assert_eq!(evaluation.operation, operation, "{provider:?}: {payload}");
            assert_eq!(
                matches!(evaluation.decision, Decision::FullBlock(_)),
                full_block,
                "{provider:?}: {payload}"
            );
            assert!(
                full_block || matches!(evaluation.decision, Decision::PartialBlock(_)),
                "{provider:?}: {payload}"
            );
        }

        let blocked = evaluate_native_hook(
            NativeHookProvider::Antigravity,
            &antigravity_payload(
                "write_to_file",
                json!({ "TargetFile": "node_modules/pkg/index.js" }),
            ),
            &codex_policy(),
        )
        .unwrap();
        let output = provider_decision_output(
            NativeHookProvider::Antigravity,
            &blocked,
            EnforcementMode::Enforce,
            "antigravity-cli",
        );
        assert_eq!(output["decision"], "deny");
        assert!(output["reason"].as_str().unwrap().contains("Do not retry"));

        let allowed = evaluate_native_hook(
            NativeHookProvider::Claude,
            &standard_payload("Read", json!({ "file_path": "src/main.rs" })),
            &codex_policy(),
        )
        .unwrap();
        let claude_allow = provider_decision_output(
            NativeHookProvider::Claude,
            &allowed,
            EnforcementMode::Enforce,
            "claude-code-cli",
        );
        assert!(
            claude_allow
                .pointer("/hookSpecificOutput/permissionDecision")
                .is_none()
        );
        let antigravity_allow = provider_decision_output(
            NativeHookProvider::Antigravity,
            &allowed,
            EnforcementMode::Enforce,
            "antigravity-cli",
        );
        assert_eq!(antigravity_allow["decision"], "ask");
    }

    #[test]
    fn codex_hook_allows_unmatched_bash() {
        let command = if cfg!(windows) {
            "Get-Content src/main.rs"
        } else {
            "cat src/main.rs"
        };
        let evaluation = evaluate_native_hook(
            NativeHookProvider::Codex,
            &codex_payload("Bash", command),
            &codex_policy(),
        )
        .unwrap();
        let output = standard_decision_output(&evaluation, EnforcementMode::Enforce, "codex-cli");
        assert_eq!(
            output.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&json!("allow"))
        );
        assert!(output.pointer("/hookSpecificOutput/updatedInput").is_none());
    }

    #[test]
    fn codex_hook_denies_protected_apply_patch() {
        let patch = "*** Begin Patch\n*** Update File: node_modules/pkg/index.js\n@@\n-old\n+new\n*** End Patch";
        let evaluation = evaluate_native_hook(
            NativeHookProvider::Codex,
            &codex_payload("apply_patch", patch),
            &codex_policy(),
        )
        .unwrap();
        let output = standard_decision_output(&evaluation, EnforcementMode::Enforce, "codex-cli");
        assert_eq!(
            output.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&json!("deny"))
        );
        assert!(
            output["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("Do not retry")
        );
    }

    #[test]
    fn codex_full_block_counts_reads_but_not_edits_as_saved_context() {
        let root = temporary_home("codex-full-block-savings");
        let policy = root.join("harnesses/codex-cli");
        fs::create_dir_all(&policy).unwrap();
        fs::write(policy.join("block-full.txt"), BALANCED_FULL).unwrap();
        fs::write(policy.join("block-partial.txt"), BALANCED_PARTIAL).unwrap();
        let config = crate::state::Config {
            mode: EnforcementMode::Enforce,
            ..Default::default()
        };
        save_config(&root, &config).unwrap();
        let store = Store::open(&root).unwrap();

        let cache = root.join("__pycache__/fixture.pyc");
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::write(&cache, vec![b'x'; 400]).unwrap();
        let read = if cfg!(windows) {
            format!("Get-Content '{}'", cache.display())
        } else {
            format!("cat '{}'", cache.display())
        };
        native_hook_dispatch(
            &root,
            &store,
            "codex-cli",
            NativeHookProvider::Codex,
            &codex_payload("Bash", &read),
        )
        .unwrap();

        let dependency = root.join("node_modules/pkg/index.js");
        fs::create_dir_all(dependency.parent().unwrap()).unwrap();
        fs::write(&dependency, vec![b'y'; 600]).unwrap();
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-old\n+new\n*** End Patch",
            dependency.display()
        );
        native_hook_dispatch(
            &root,
            &store,
            "codex-cli",
            NativeHookProvider::Codex,
            &codex_payload("apply_patch", &patch),
        )
        .unwrap();

        let (protected, avoided, replacement, retry, tokens) = store.savings().unwrap();
        assert_eq!(
            (protected, avoided, replacement, retry, tokens),
            (1000, 400, 0, 0, 100)
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_hook_returns_partial_context_but_shadow_allows_original() {
        let command = if cfg!(windows) {
            "Get-Content node_modules/pkg/index.js"
        } else {
            "cat node_modules/pkg/index.js"
        };
        let evaluation = evaluate_native_hook(
            NativeHookProvider::Codex,
            &codex_payload("Bash", command),
            &codex_policy(),
        )
        .unwrap();
        let partial = match &evaluation.decision {
            Decision::PartialBlock(partial) => partial,
            other => panic!("expected partial block, got {other:?}"),
        };
        let context = partial_context(partial, "bounded result");
        let enforce = standard_partial_output(partial, &context);
        assert_eq!(
            enforce.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&json!("deny"))
        );
        assert_eq!(
            enforce.pointer("/hookSpecificOutput/additionalContext"),
            Some(&json!(context))
        );
        assert!(context.contains("Bound dependency reads"));
        assert!(context.contains("Do not retry or work around this block"));
        assert!(context.ends_with("bounded result"));

        let shadow = standard_decision_output(&evaluation, EnforcementMode::Shadow, "codex-cli");
        assert_eq!(
            shadow.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&json!("allow"))
        );
        assert!(shadow.pointer("/hookSpecificOutput/updatedInput").is_none());
    }

    #[test]
    fn codex_hook_keeps_first_equal_priority_resource() {
        let command = if cfg!(windows) {
            "Get-Content node_modules/first.js; Get-Content node_modules/second.js"
        } else {
            "cat node_modules/first.js; cat node_modules/second.js"
        };
        let evaluation = evaluate_native_hook(
            NativeHookProvider::Codex,
            &codex_payload("Bash", command),
            &codex_policy(),
        )
        .unwrap();
        assert_eq!(evaluation.resource, "node_modules/first.js");
    }

    #[test]
    fn bounded_substitute_reports_actual_output_bytes() {
        let root = temporary_home("replacement-bytes");
        let path = root.join("sample.txt");
        fs::write(&path, "hello\n").unwrap();
        let mut policy = PolicySet::new();
        policy.add(
            CompiledPolicy::parse_partial(
                "**/* | read | limit | max_lines=20,max_bytes=4096 | Bound reads",
            )
            .unwrap(),
            RuleScope::Harness,
        );
        let partial = match policy.decide(Operation::Read, &path.to_string_lossy()) {
            Decision::PartialBlock(partial) => partial,
            other => panic!("expected partial block, got {other:?}"),
        };
        let bytes = run_substitute(&path.to_string_lossy(), &partial, None, None).unwrap();
        assert!(bytes >= 6);
        assert!(bytes < 4096);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_codex_hook_input_has_fail_closed_output() {
        let malformed = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {}
        });
        let error = evaluate_native_hook(NativeHookProvider::Codex, &malformed, &codex_policy())
            .unwrap_err();
        assert_eq!(
            standard_error_output(&error).pointer("/hookSpecificOutput/permissionDecision"),
            Some(&json!("deny"))
        );
    }
}
