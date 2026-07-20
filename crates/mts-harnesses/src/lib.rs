#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IntegrationFamily {
    NativeHook,
    NativePlugin,
    SdkMiddleware,
    AcpProxy,
    ProcessWrapper,
    SandboxWorkspace,
    IdeCompanion,
    RepoBootstrap,
    TelemetryProxy,
    CustomCommand,
}

impl IntegrationFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeHook => "NATIVE_HOOK",
            Self::NativePlugin => "NATIVE_PLUGIN",
            Self::SdkMiddleware => "SDK_MIDDLEWARE",
            Self::AcpProxy => "ACP_PROXY",
            Self::ProcessWrapper => "PROCESS_WRAPPER",
            Self::SandboxWorkspace => "SANDBOX_WORKSPACE",
            Self::IdeCompanion => "IDE_COMPANION",
            Self::RepoBootstrap => "REPO_BOOTSTRAP",
            Self::TelemetryProxy => "TELEMETRY_PROXY",
            Self::CustomCommand => "CUSTOM_COMMAND",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CapabilityGrade {
    Strict,
    Strong,
    Partial,
    Advisory,
    ObserveOnly,
    Unverified,
}

impl CapabilityGrade {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "STRICT",
            Self::Strong => "STRONG",
            Self::Partial => "PARTIAL",
            Self::Advisory => "ADVISORY",
            Self::ObserveOnly => "OBSERVE_ONLY",
            Self::Unverified => "UNVERIFIED",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectionMode {
    Shadow,
    Warn,
    Enforce,
}

impl ProtectionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shadow => "SHADOW",
            Self::Warn => "WARN",
            Self::Enforce => "ENFORCE",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityRange {
    pub maximum: CapabilityGrade,
    pub fallback: Option<CapabilityGrade>,
    pub planned_label: &'static str,
}

impl CapabilityRange {
    pub const fn label(self) -> &'static str {
        self.planned_label
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DetectionMetadata {
    pub commands: &'static [&'static str],
    pub markers: &'static [&'static str],
    pub version_args: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterMetadata {
    pub installation_form: &'static str,
    pub policy_dir: &'static str,
    pub owner: &'static str,
    pub default_mode: ProtectionMode,
    pub install_dry_run: bool,
    pub install: bool,
    pub uninstall: bool,
    pub owned_files: &'static [&'static str],
    pub fixture_dir: &'static str,
    pub doctor_template: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    pub id: &'static str,
    pub display_name: &'static str,
    pub execution_surface: &'static str,
    pub families: &'static [IntegrationFamily],
    pub capability: CapabilityRange,
    pub detection: DetectionMetadata,
    pub adapter: AdapterMetadata,
}

const S: CapabilityRange = CapabilityRange {
    maximum: CapabilityGrade::Strict,
    fallback: None,
    planned_label: "STRICT",
};
const G: CapabilityRange = CapabilityRange {
    maximum: CapabilityGrade::Strong,
    fallback: None,
    planned_label: "STRONG",
};
const P: CapabilityRange = CapabilityRange {
    maximum: CapabilityGrade::Partial,
    fallback: None,
    planned_label: "PARTIAL",
};
const A: CapabilityRange = CapabilityRange {
    maximum: CapabilityGrade::Advisory,
    fallback: None,
    planned_label: "ADVISORY",
};
const GP: CapabilityRange = CapabilityRange {
    maximum: CapabilityGrade::Strong,
    fallback: Some(CapabilityGrade::Partial),
    planned_label: "STRONG or PARTIAL",
};
const PA: CapabilityRange = CapabilityRange {
    maximum: CapabilityGrade::Partial,
    fallback: Some(CapabilityGrade::Advisory),
    planned_label: "PARTIAL or ADVISORY",
};
const AP: CapabilityRange = CapabilityRange {
    maximum: CapabilityGrade::Partial,
    fallback: Some(CapabilityGrade::Advisory),
    planned_label: "ADVISORY or PARTIAL",
};

macro_rules! target {
    ($id:literal, $name:literal, $surface:literal, [$($family:ident),+], $install:literal, $grade:expr, [$($command:literal),*], [$($marker:literal),*]) => {
        Target {
            id: $id,
            display_name: $name,
            execution_surface: $surface,
            families: &[$(IntegrationFamily::$family),+],
            capability: $grade,
            detection: DetectionMetadata {
                commands: &[$($command),*],
                markers: &[$($marker),*],
                version_args: &["--version"],
            },
            adapter: AdapterMetadata {
                installation_form: $install,
                policy_dir: concat!("~/.mts/harnesses/", $id),
                owner: "MTS adapter maintainers",
                default_mode: ProtectionMode::Shadow,
                install_dry_run: true,
                install: true,
                uninstall: true,
                owned_files: &["block-full.txt", "block-partial.txt", "adapter.json", "install-manifest.json"],
                fixture_dir: concat!("fixtures/contracts/", $id),
                doctor_template: "Detection: {detection}\nVersion: {version}\nIntegration: {integration}\nPolicy files: {policy_files}\nRecommended mode: SHADOW",
            },
        }
    };
}

pub static TARGETS: [Target; 64] = [
    target!(
        "claude-code-cli",
        "Claude Code CLI",
        "Claude Code CLI",
        [NativeHook],
        "User or project hook configuration",
        S,
        ["claude"],
        ["claude-code"]
    ),
    target!(
        "claude-code-vscode",
        "Claude Code for VS Code",
        "Claude Code for VS Code",
        [NativeHook],
        "Shared Claude settings plus IDE probe",
        G,
        [],
        ["claude-code-vscode"]
    ),
    target!(
        "claude-code-jetbrains",
        "Claude Code for JetBrains",
        "Claude Code for JetBrains",
        [NativeHook],
        "Shared Claude settings plus IDE probe",
        G,
        [],
        ["claude-code-jetbrains"]
    ),
    target!(
        "codex-cli",
        "OpenAI Codex CLI",
        "OpenAI Codex CLI",
        [NativeHook],
        "Native hooks and configuration",
        G,
        ["codex"],
        ["codex-cli"]
    ),
    target!(
        "codex-vscode",
        "OpenAI Codex for VS Code",
        "OpenAI Codex for VS Code",
        [NativeHook],
        "CLI or app-server hook reuse",
        G,
        [],
        ["codex-vscode"]
    ),
    target!(
        "antigravity-cli",
        "Google Antigravity CLI",
        "Google Antigravity CLI",
        [NativeHook],
        "Global Antigravity hook configuration",
        S,
        ["agy"],
        ["antigravity-cli"]
    ),
    target!(
        "gemini-cli",
        "Gemini CLI",
        "Gemini CLI",
        [NativeHook],
        "BeforeTool and AfterTool hooks",
        S,
        ["gemini"],
        ["gemini-cli"]
    ),
    target!(
        "qwen-code-cli",
        "Qwen Code CLI",
        "Qwen Code CLI",
        [NativeHook],
        "Native hook settings",
        S,
        ["qwen", "qwen-code"],
        ["qwen-code-cli"]
    ),
    target!(
        "copilot-cli",
        "GitHub Copilot CLI",
        "GitHub Copilot CLI",
        [NativeHook],
        "User or repository hook configuration",
        S,
        ["copilot"],
        ["copilot-cli"]
    ),
    target!(
        "opencode-cli",
        "OpenCode CLI",
        "OpenCode CLI",
        [NativePlugin],
        "Local TypeScript plugin",
        S,
        ["opencode"],
        ["opencode-cli"]
    ),
    target!(
        "cline-cli",
        "Cline CLI",
        "Cline CLI",
        [SdkMiddleware],
        "CLI or SDK middleware",
        G,
        ["cline"],
        ["cline-cli"]
    ),
    target!(
        "cline-vscode",
        "Cline for VS Code",
        "Cline for VS Code",
        [SdkMiddleware],
        "Extension bridge",
        G,
        [],
        ["cline-vscode"]
    ),
    target!(
        "cline-jetbrains",
        "Cline for JetBrains",
        "Cline for JetBrains",
        [SdkMiddleware],
        "Shared core bridge",
        G,
        [],
        ["cline-jetbrains"]
    ),
    target!(
        "goose-cli",
        "Goose CLI",
        "Goose CLI",
        [NativePlugin],
        "Extension or tool middleware",
        G,
        ["goose"],
        ["goose-cli"]
    ),
    target!(
        "goose-desktop",
        "Goose Desktop",
        "Goose Desktop",
        [NativePlugin],
        "Extension configuration",
        G,
        [],
        ["goose-desktop"]
    ),
    target!(
        "gptme-cli",
        "gptme CLI",
        "gptme CLI",
        [NativePlugin],
        "gptme plugin",
        G,
        ["gptme"],
        ["gptme-cli"]
    ),
    target!(
        "gptme-acp",
        "gptme ACP Server",
        "gptme ACP Server",
        [AcpProxy],
        "ACP launcher replacement",
        G,
        [],
        ["gptme-acp"]
    ),
    target!(
        "hermes-cli",
        "Hermes Agent CLI and TUI",
        "Hermes Agent CLI and TUI",
        [NativePlugin, SdkMiddleware],
        "Hermes plugin and tool middleware",
        G,
        ["hermes"],
        ["hermes-cli"]
    ),
    target!(
        "hermes-gateway",
        "Hermes Messaging Gateway",
        "Hermes Messaging Gateway",
        [SdkMiddleware],
        "Gateway host plugin",
        G,
        ["hermes-gateway"],
        ["hermes-gateway"]
    ),
    target!(
        "openhands-local",
        "OpenHands Local Runtime",
        "OpenHands Local Runtime",
        [SdkMiddleware],
        "Action and event middleware",
        G,
        ["openhands"],
        ["openhands-local"]
    ),
    target!(
        "continue-cli",
        "Continue CLI or Headless",
        "Continue CLI or Headless",
        [SdkMiddleware],
        "Core and tool wrapper",
        G,
        ["cn", "continue"],
        ["continue-cli"]
    ),
    target!(
        "kimi-code-cli",
        "Kimi Code CLI",
        "Kimi Code CLI",
        [AcpProxy, ProcessWrapper],
        "ACP proxy and CLI launcher",
        GP,
        ["kimi", "kimi-code"],
        ["kimi-code-cli"]
    ),
    target!(
        "kimi-code-vscode",
        "Kimi Code for VS Code",
        "Kimi Code for VS Code",
        [AcpProxy, IdeCompanion],
        "Extension launcher and configuration",
        P,
        [],
        ["kimi-code-vscode"]
    ),
    target!(
        "qwen-code-acp",
        "Qwen Code ACP or Daemon",
        "Qwen Code ACP or Daemon",
        [AcpProxy],
        "ACP command replacement",
        G,
        [],
        ["qwen-code-acp"]
    ),
    target!(
        "aider",
        "Aider",
        "Aider",
        [ProcessWrapper],
        "Executable wrapper and ignore export",
        P,
        ["aider"],
        ["aider"]
    ),
    target!(
        "swe-agent",
        "SWE-agent",
        "SWE-agent",
        [SdkMiddleware, SandboxWorkspace],
        "Environment and tool wrapper",
        G,
        ["sweagent", "swe-agent"],
        ["swe-agent"]
    ),
    target!(
        "plandex",
        "Plandex",
        "Plandex",
        [ProcessWrapper],
        "CLI wrapper and filtered context",
        P,
        ["plandex"],
        ["plandex"]
    ),
    target!(
        "open-interpreter",
        "Open Interpreter",
        "Open Interpreter",
        [SdkMiddleware],
        "Computer and shell tool middleware",
        G,
        ["interpreter"],
        ["open-interpreter"]
    ),
    target!(
        "gpt-pilot",
        "GPT-Pilot",
        "GPT-Pilot",
        [ProcessWrapper],
        "CLI wrapper and workspace policy",
        P,
        ["gpt-pilot"],
        ["gpt-pilot"]
    ),
    target!(
        "auto-code-rover",
        "AutoCodeRover",
        "AutoCodeRover",
        [SdkMiddleware],
        "Runner and tool wrapper",
        G,
        ["auto-code-rover", "acr"],
        ["auto-code-rover"]
    ),
    target!(
        "smol-developer",
        "Smol Developer",
        "Smol Developer",
        [SdkMiddleware],
        "Embedded agent middleware",
        G,
        ["smol-developer"],
        ["smol-developer"]
    ),
    target!(
        "codebuff",
        "Codebuff",
        "Codebuff",
        [ProcessWrapper],
        "CLI wrapper and ignore export",
        P,
        ["codebuff"],
        ["codebuff"]
    ),
    target!(
        "freebuff",
        "Freebuff",
        "Freebuff",
        [ProcessWrapper],
        "CLI wrapper and ignore export",
        P,
        ["freebuff"],
        ["freebuff"]
    ),
    target!(
        "crush",
        "Charmbracelet Crush",
        "Charmbracelet Crush",
        [ProcessWrapper],
        "CLI wrapper and sandbox",
        P,
        ["crush"],
        ["crush"]
    ),
    target!(
        "amazon-q-cli",
        "Amazon Q Developer CLI",
        "Amazon Q Developer CLI",
        [ProcessWrapper],
        "Approval probe and launcher",
        P,
        ["q", "amazon-q"],
        ["amazon-q-cli"]
    ),
    target!(
        "rovo-dev-cli",
        "Atlassian Rovo Dev CLI",
        "Atlassian Rovo Dev CLI",
        [ProcessWrapper],
        "CLI launcher and repository policy",
        P,
        ["rovodev", "rovo"],
        ["rovo-dev-cli"]
    ),
    target!(
        "amp-cli",
        "Amp CLI",
        "Amp CLI",
        [ProcessWrapper],
        "CLI launcher and workspace isolation",
        P,
        ["amp"],
        ["amp-cli"]
    ),
    target!(
        "factory-droid-cli",
        "Factory Droid CLI",
        "Factory Droid CLI",
        [ProcessWrapper],
        "CLI launcher and configuration probe",
        P,
        ["droid"],
        ["factory-droid-cli"]
    ),
    target!(
        "pi-coding-agent",
        "Pi Coding Agent",
        "Pi Coding Agent",
        [ProcessWrapper],
        "CLI launcher and ACP probe",
        P,
        ["pi"],
        ["pi-coding-agent"]
    ),
    target!(
        "openclaw",
        "OpenClaw Local Agent",
        "OpenClaw Local Agent",
        [NativePlugin, SandboxWorkspace],
        "Gateway or plugin execution policy",
        G,
        ["openclaw"],
        ["openclaw"]
    ),
    target!(
        "warp-agent",
        "Warp Agent",
        "Warp Agent",
        [ProcessWrapper, IdeCompanion],
        "Terminal launch profile",
        P,
        ["warp"],
        ["warp-agent"]
    ),
    target!(
        "mentat",
        "Mentat",
        "Mentat",
        [ProcessWrapper],
        "Python entrypoint wrapper",
        P,
        ["mentat"],
        ["mentat"]
    ),
    target!(
        "custom-command",
        "Custom Local Harness",
        "Custom Local Harness",
        [CustomCommand],
        "User-supplied command manifest",
        PA,
        [],
        ["custom-command"]
    ),
    target!(
        "cursor-ide",
        "Cursor Agent",
        "Cursor Agent",
        [IdeCompanion, NativeHook],
        "User or project installation",
        GP,
        ["cursor"],
        ["cursor-ide"]
    ),
    target!(
        "cursor-background",
        "Cursor Background Agent",
        "Cursor Background Agent",
        [RepoBootstrap],
        "Repository bootstrap",
        A,
        [],
        ["cursor-background"]
    ),
    target!(
        "windsurf-cascade",
        "Windsurf Cascade",
        "Windsurf Cascade",
        [IdeCompanion],
        "Workspace rules and terminal wrapper",
        P,
        ["windsurf"],
        ["windsurf-cascade"]
    ),
    target!(
        "roo-code",
        "Roo Code",
        "Roo Code",
        [IdeCompanion],
        "VS Code companion and rules",
        P,
        [],
        ["roo-code"]
    ),
    target!(
        "kilo-code",
        "Kilo Code",
        "Kilo Code",
        [IdeCompanion],
        "VS Code companion and rules",
        P,
        [],
        ["kilo-code"]
    ),
    target!(
        "continue-ide",
        "Continue for VS Code or JetBrains",
        "Continue for VS Code or JetBrains",
        [SdkMiddleware],
        "Extension and tool configuration",
        GP,
        [],
        ["continue-ide"]
    ),
    target!(
        "zed-agent-panel",
        "Zed Agent Panel",
        "Zed Agent Panel",
        [AcpProxy, IdeCompanion],
        "Agent server configuration",
        GP,
        ["zed"],
        ["zed-agent-panel"]
    ),
    target!(
        "jetbrains-acp",
        "JetBrains ACP Agent Panel",
        "JetBrains ACP Agent Panel",
        [AcpProxy],
        "ACP command proxy",
        GP,
        [],
        ["jetbrains-acp"]
    ),
    target!(
        "jetbrains-junie",
        "JetBrains Junie",
        "JetBrains Junie",
        [IdeCompanion],
        "Project rules and terminal wrapper",
        P,
        [],
        ["jetbrains-junie"]
    ),
    target!(
        "copilot-vscode-agent",
        "GitHub Copilot VS Code Agent Mode",
        "GitHub Copilot VS Code Agent Mode",
        [IdeCompanion],
        "Extension settings and repository hook",
        P,
        [],
        ["copilot-vscode-agent"]
    ),
    target!(
        "amazon-q-ide",
        "Amazon Q Developer IDE",
        "Amazon Q Developer IDE",
        [IdeCompanion],
        "IDE workspace policy",
        P,
        [],
        ["amazon-q-ide"]
    ),
    target!(
        "kiro-ide",
        "Amazon Kiro IDE",
        "Amazon Kiro IDE",
        [IdeCompanion],
        "Steering configuration and shell wrapper",
        P,
        ["kiro"],
        ["kiro-ide"]
    ),
    target!(
        "sourcegraph-cody",
        "Sourcegraph Cody",
        "Sourcegraph Cody",
        [IdeCompanion],
        "Enterprise policy export",
        AP,
        [],
        ["sourcegraph-cody"]
    ),
    target!(
        "augment-code",
        "Augment Code",
        "Augment Code",
        [IdeCompanion],
        "Workspace configuration and shell wrapper",
        AP,
        [],
        ["augment-code"]
    ),
    target!(
        "refact-ai",
        "Refact.ai",
        "Refact.ai",
        [IdeCompanion],
        "Extension configuration and plugin probe",
        P,
        [],
        ["refact-ai"]
    ),
    target!(
        "tabby",
        "Tabby",
        "Tabby",
        [IdeCompanion, TelemetryProxy],
        "IDE and server configuration",
        A,
        ["tabby"],
        ["tabby"]
    ),
    target!(
        "void-editor",
        "Void Editor",
        "Void Editor",
        [IdeCompanion],
        "Workspace policy and terminal wrapper",
        P,
        ["void"],
        ["void-editor"]
    ),
    target!(
        "pearai",
        "PearAI",
        "PearAI",
        [IdeCompanion],
        "Workspace policy and terminal wrapper",
        P,
        ["pearai"],
        ["pearai"]
    ),
    target!(
        "trae",
        "Trae",
        "Trae",
        [IdeCompanion],
        "Workspace policy and shell wrapper",
        AP,
        ["trae"],
        ["trae"]
    ),
    target!(
        "codebuddy",
        "CodeBuddy",
        "CodeBuddy",
        [IdeCompanion],
        "Workspace policy and shell wrapper",
        A,
        ["codebuddy"],
        ["codebuddy"]
    ),
    target!(
        "blackbox-ai",
        "Blackbox AI IDE or CLI",
        "Blackbox AI IDE or CLI",
        [IdeCompanion, ProcessWrapper],
        "Surface-specific installation",
        AP,
        ["blackbox"],
        ["blackbox-ai"]
    ),
];

pub fn registry() -> &'static [Target] {
    &TARGETS
}

pub fn list_targets() -> impl ExactSizeIterator<Item = &'static Target> {
    TARGETS.iter()
}

pub fn target_by_id(id: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|target| target.id == id)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DetectionEvidence<'a> {
    pub commands: &'a [&'a str],
    pub markers: &'a [&'a str],
}

pub fn detect_targets(evidence: &DetectionEvidence<'_>) -> Vec<&'static Target> {
    let commands: HashSet<String> = evidence
        .commands
        .iter()
        .map(|value| command_name(value))
        .collect();
    let markers: HashSet<String> = evidence
        .markers
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect();
    TARGETS
        .iter()
        .filter(|target| {
            target
                .detection
                .commands
                .iter()
                .any(|candidate| commands.contains(*candidate))
                || target
                    .detection
                    .markers
                    .iter()
                    .any(|candidate| markers.contains(*candidate))
        })
        .collect()
}

fn command_name(value: &str) -> String {
    let file_name = value.rsplit(['/', '\\']).next().unwrap_or(value);
    let lower = file_name.to_ascii_lowercase();
    [".exe", ".cmd", ".bat", ".ps1"]
        .iter()
        .find_map(|suffix| lower.strip_suffix(suffix))
        .unwrap_or(&lower)
        .to_owned()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownTarget(pub String);

impl fmt::Display for UnknownTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Unknown harness target: {}", self.0)
    }
}

impl std::error::Error for UnknownTarget {}

pub fn select_targets(ids: &[&str]) -> Result<Vec<&'static Target>, UnknownTarget> {
    if ids.is_empty() || ids == ["all"] {
        return Ok(TARGETS.iter().collect());
    }
    let requested: HashSet<&str> = ids.iter().copied().collect();
    if let Some(id) = requested.iter().find(|id| target_by_id(id).is_none()) {
        return Err(UnknownTarget((*id).to_owned()));
    }
    Ok(TARGETS
        .iter()
        .filter(|target| requested.contains(target.id))
        .collect())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionAssessment {
    pub verified: bool,
    pub grade: CapabilityGrade,
    pub mode: ProtectionMode,
    pub reason: &'static str,
}

pub fn assess_version(
    target: &Target,
    detected_version: Option<&str>,
    verified_versions: &[&str],
    requested_mode: ProtectionMode,
) -> VersionAssessment {
    if detected_version.is_some_and(|version| verified_versions.contains(&version)) {
        VersionAssessment {
            verified: true,
            grade: target.capability.maximum,
            mode: requested_mode,
            reason: "The detected version has a verified contract.",
        }
    } else {
        VersionAssessment {
            verified: false,
            grade: CapabilityGrade::Unverified,
            mode: ProtectionMode::Shadow,
            reason: "The detected version is unknown; protection remains in SHADOW mode.",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactKind {
    PhysicalTextFile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyArtifact {
    pub destination: String,
    pub contents: String,
    pub kind: ArtifactKind,
}

pub fn render_policy_artifacts(
    target: &Target,
    full: &str,
    partial: &str,
) -> io::Result<[PolicyArtifact; 2]> {
    if full.contains('\0') || partial.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Policy text contains a NUL byte.",
        ));
    }
    Ok([
        PolicyArtifact {
            destination: format!("{}/block-full.txt", target.adapter.policy_dir),
            contents: canonical_text(full),
            kind: ArtifactKind::PhysicalTextFile,
        },
        PolicyArtifact {
            destination: format!("{}/block-partial.txt", target.adapter.policy_dir),
            contents: canonical_text(partial),
            kind: ArtifactKind::PhysicalTextFile,
        },
    ])
}

fn canonical_text(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!("{}\n", value.trim_end_matches(['\r', '\n']))
    }
}

pub fn write_policy_files(
    harness_dir: &Path,
    full: &str,
    partial: &str,
) -> io::Result<[PathBuf; 2]> {
    if harness_dir
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Harness policy directory must not be a symbolic link.",
        ));
    }
    if full.contains('\0') || partial.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Policy text contains a NUL byte.",
        ));
    }
    fs::create_dir_all(harness_dir)?;
    let destinations = [
        harness_dir.join("block-full.txt"),
        harness_dir.join("block-partial.txt"),
    ];
    fs::write(&destinations[0], canonical_text(full))?;
    fs::write(&destinations[1], canonical_text(partial))?;
    Ok(destinations)
}

pub fn support_matrix_markdown() -> String {
    let mut output = String::from("| Target | Execution surface | Integration | Maximum grade | Default mode |\n|---|---|---|---|---|\n");
    for target in TARGETS {
        let families = target
            .families
            .iter()
            .map(|family| family.as_str())
            .collect::<Vec<_>>()
            .join(" + ");
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            target.id,
            target.execution_surface,
            families,
            target.capability.label(),
            target.adapter.default_mode.as_str(),
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn registry_has_exact_planned_targets_and_required_metadata() {
        assert_eq!(TARGETS.len(), 64);
        let ids: HashSet<_> = TARGETS.iter().map(|target| target.id).collect();
        assert_eq!(ids.len(), TARGETS.len());
        for target in TARGETS {
            assert!(!target.id.is_empty());
            assert!(!target.display_name.is_empty());
            assert!(!target.execution_surface.is_empty());
            assert!(!target.families.is_empty());
            assert!(!target.adapter.installation_form.is_empty());
            assert_eq!(target.adapter.default_mode, ProtectionMode::Shadow);
            assert_eq!(
                target.adapter.policy_dir,
                format!("~/.mts/harnesses/{}", target.id)
            );
            assert!(!target.adapter.owner.is_empty());
            assert!(target.adapter.install_dry_run);
            assert!(target.adapter.install);
            assert!(target.adapter.uninstall);
            assert!(target.adapter.owned_files.contains(&"block-full.txt"));
            assert!(target.adapter.owned_files.contains(&"block-partial.txt"));
            assert_eq!(
                target.adapter.fixture_dir,
                format!("fixtures/contracts/{}", target.id)
            );
            assert!(!target.adapter.doctor_template.is_empty());
        }
        for required in [
            "claude-code-cli",
            "codex-cli",
            "antigravity-cli",
            "kimi-code-cli",
            "custom-command",
            "blackbox-ai",
        ] {
            assert!(ids.contains(required));
        }
    }

    #[test]
    fn canonical_registry_and_matrix_strings_are_english_ascii() {
        let matrix = support_matrix_markdown();
        assert!(matrix.is_ascii());
        for target in TARGETS {
            assert!(target.id.is_ascii());
            assert!(target.display_name.is_ascii());
            assert!(target.execution_surface.is_ascii());
            assert!(target.adapter.installation_form.is_ascii());
            assert!(target.adapter.policy_dir.is_ascii());
            assert!(target.adapter.owner.is_ascii());
        }
    }

    #[test]
    fn detection_and_selection_are_data_driven() {
        let detected = detect_targets(&DetectionEvidence {
            commands: &[r"C:\Tools\codex.EXE", "/usr/local/bin/aider"],
            markers: &["kimi-code-vscode"],
        });
        let ids: HashSet<_> = detected.iter().map(|target| target.id).collect();
        assert!(ids.contains("codex-cli"));
        assert!(ids.contains("aider"));
        assert!(ids.contains("kimi-code-vscode"));

        let selected = select_targets(&["aider", "codex-cli", "aider"]).unwrap();
        assert_eq!(
            selected.iter().map(|target| target.id).collect::<Vec<_>>(),
            ["codex-cli", "aider"]
        );
        assert!(select_targets(&["missing-target"]).is_err());
    }

    #[test]
    fn unknown_versions_are_honestly_forced_to_shadow() {
        let target = target_by_id("gemini-cli").unwrap();
        let unknown = assess_version(target, Some("99.0.0"), &["1.0.0"], ProtectionMode::Enforce);
        assert_eq!(unknown.grade, CapabilityGrade::Unverified);
        assert_eq!(unknown.mode, ProtectionMode::Shadow);
        assert!(!unknown.verified);

        let known = assess_version(target, Some("1.0.0"), &["1.0.0"], ProtectionMode::Enforce);
        assert_eq!(known.grade, CapabilityGrade::Strict);
        assert_eq!(known.mode, ProtectionMode::Enforce);
        assert!(known.verified);
    }

    #[test]
    fn policy_artifacts_are_physical_text_files_not_links() {
        let target = target_by_id("codex-cli").unwrap();
        let rendered = render_policy_artifacts(
            target,
            "node_modules/** | write",
            "**/*.log | read | errors-only",
        )
        .unwrap();
        assert_eq!(
            rendered[0].destination,
            "~/.mts/harnesses/codex-cli/block-full.txt"
        );
        assert_eq!(
            rendered[1].destination,
            "~/.mts/harnesses/codex-cli/block-partial.txt"
        );
        assert!(rendered
            .iter()
            .all(|artifact| artifact.kind == ArtifactKind::PhysicalTextFile));

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("mts-harnesses-{unique}"));
        let paths = write_policy_files(&directory, "full", "partial").unwrap();
        for path in paths {
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(metadata.file_type().is_file());
            assert!(!metadata.file_type().is_symlink());
            assert_eq!(
                path.extension().and_then(|value| value.to_str()),
                Some("txt")
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
