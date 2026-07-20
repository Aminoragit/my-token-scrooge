use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "mts",
    version,
    about = "Stop AI agents from eating your context."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Setup(SetupArgs),
    Doctor,
    Status,
    Uninstall(UninstallArgs),
    Mode(ModeArgs),
    Project(ProjectArgs),
    Policy(PolicyArgs),
    Harness(HarnessArgs),
    Simulate(SimulateArgs),
    Retries(RetriesArgs),
    AllowOnce(AllowOnceArgs),
    Savings(SavingsArgs),
    Report(ReportArgs),
    Benchmark(BenchmarkArgs),
    #[command(hide = true)]
    Hook(PassthroughArgs),
    #[command(hide = true)]
    AcpProxy(PassthroughArgs),
    #[command(hide = true)]
    Wrapper(PassthroughArgs),
    #[command(hide = true)]
    Worker(PassthroughArgs),
    #[command(hide = true)]
    RepoBootstrap(PassthroughArgs),
    SelfManage(SelfArgs),
}

#[derive(Args, Debug)]
pub struct SetupArgs {
    #[arg(long, default_value = "balanced")]
    pub profile: String,
    #[arg(long, value_delimiter = ',')]
    pub targets: Vec<String>,
    #[arg(long)]
    pub yes: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long, hide = true)]
    pub codex_home: Option<PathBuf>,
    #[command(subcommand)]
    pub custom: Option<CustomSetup>,
}

#[derive(Subcommand, Debug)]
pub enum CustomSetup {
    Custom {
        #[arg(long)]
        id: String,
        #[arg(long)]
        command: String,
        #[arg(long)]
        workspace_arg: Option<String>,
        #[arg(long, default_value = "wrapper")]
        mode: String,
    },
}

#[derive(Args, Debug)]
pub struct UninstallArgs {
    #[arg(long)]
    pub purge: bool,
    #[arg(long, value_delimiter = ',')]
    pub targets: Vec<String>,
}

#[derive(Args, Debug)]
pub struct ModeArgs {
    pub mode: Option<String>,
}

#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    Init {
        #[arg(long, default_value = "overlay")]
        mode: String,
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

#[derive(Args, Debug)]
pub struct PolicyArgs {
    #[command(subcommand)]
    pub command: PolicyCommand,
}

#[derive(Subcommand, Debug)]
pub enum PolicyCommand {
    List(PolicyScope),
    Add {
        kind: PolicyKind,
        rule: String,
        #[command(flatten)]
        scope: Scope,
    },
    Edit {
        kind: PolicyKind,
        rule_id: String,
        replacement: Option<String>,
        #[command(flatten)]
        scope: Scope,
    },
    Remove {
        kind: PolicyKind,
        rule_id: String,
        #[command(flatten)]
        scope: Scope,
    },
    Validate(PolicyScope),
    Format(PolicyScope),
}

#[derive(Args, Debug)]
pub struct PolicyScope {
    pub kind: Option<PolicyKind>,
    #[command(flatten)]
    pub scope: Scope,
}

#[derive(Args, Debug, Default)]
pub struct Scope {
    #[arg(long, conflicts_with_all = ["targets", "project"])]
    pub target: Option<String>,
    #[arg(long, value_delimiter = ',', conflicts_with = "project")]
    pub targets: Vec<String>,
    #[arg(long)]
    pub project: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum PolicyKind {
    Full,
    Partial,
}

#[derive(Args, Debug)]
pub struct HarnessArgs {
    #[command(subcommand)]
    pub command: HarnessCommand,
}

#[derive(Subcommand, Debug)]
pub enum HarnessCommand {
    Detect,
    List {
        #[arg(long, default_value = "table")]
        format: String,
    },
    Install {
        target: String,
    },
    Uninstall {
        target: String,
    },
    Verify {
        target: String,
    },
    Drift,
    Sync {
        #[arg(long)]
        from: String,
        #[arg(long, value_delimiter = ',')]
        to: Vec<String>,
    },
}

#[derive(Args, Debug)]
pub struct SimulateArgs {
    #[arg(value_enum)]
    pub operation: OperationArg,
    pub input: String,
    #[arg(long, default_value = "codex-cli")]
    pub target: String,
    #[arg(long, default_value = "default")]
    pub session: String,
    #[arg(long)]
    pub diff: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OperationArg {
    Read,
    Write,
    Edit,
    Search,
    Shell,
    Execute,
    Mcp,
}

#[derive(Args, Debug)]
pub struct RetriesArgs {
    #[command(subcommand)]
    pub command: RetryCommand,
}

#[derive(Subcommand, Debug)]
pub enum RetryCommand {
    List,
    Show { intent_id: String },
    Unlock { intent_id: String },
    Lock { intent_id: String },
}

#[derive(Args, Debug)]
pub struct AllowOnceArgs {
    pub request_id: String,
    #[arg(long, default_value = "default")]
    pub session: String,
    #[arg(long, default_value = "read")]
    pub operation: String,
}

#[derive(Args, Debug)]
pub struct SavingsArgs {
    pub period: Option<String>,
    #[arg(long)]
    pub target: Option<String>,
    #[arg(long)]
    pub project: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ReportArgs {
    #[command(subcommand)]
    pub command: ReportCommand,
}

#[derive(Subcommand, Debug)]
pub enum ReportCommand {
    Export {
        #[arg(long, default_value = "markdown")]
        format: String,
    },
}

#[derive(Args, Debug)]
pub struct BenchmarkArgs {
    #[command(subcommand)]
    pub command: BenchmarkCommand,
}

#[derive(Subcommand, Debug)]
pub enum BenchmarkCommand {
    Run {
        #[arg(long)]
        suite: Option<String>,
    },
    Compare,
    Export,
}

#[derive(Args, Debug)]
pub struct PassthroughArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub arguments: Vec<String>,
}

#[derive(Args, Debug)]
pub struct SelfArgs {
    #[command(subcommand)]
    pub command: SelfCommand,
}

#[derive(Subcommand, Debug)]
pub enum SelfCommand {
    Rollback,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_documented_policy_command() {
        let cli = Cli::try_parse_from([
            "mts",
            "policy",
            "add",
            "full",
            "node_modules/** | write,edit | Installed dependencies must not be modified directly",
            "--targets",
            "all",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::Policy(_))));
    }
}
