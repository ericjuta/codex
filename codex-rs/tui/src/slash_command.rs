use strum::IntoEnumIterator;
use strum_macros::AsRefStr;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

/// Commands that can be invoked by starting a message with a leading slash.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, EnumIter, AsRefStr, IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SlashCommand {
    // DO NOT ALPHA-SORT! Enum order is presentation order in the popup, so
    // more frequently used commands should be listed first.
    Model,
    Fast,
    Approvals,
    Permissions,
    #[strum(serialize = "setup-default-sandbox")]
    ElevateSandbox,
    #[strum(serialize = "sandbox-add-read-dir")]
    SandboxReadRoot,
    Experimental,
    Skills,
    Review,
    Rename,
    New,
    Resume,
    Fork,
    Init,
    Compact,
    Plan,
    Collab,
    Agent,
    // Undo,
    Copy,
    Diff,
    Mention,
    Status,
    DebugConfig,
    Title,
    Statusline,
    Theme,
    Mcp,
    Apps,
    Plugins,
    Logout,
    Quit,
    Exit,
    Feedback,
    Rollout,
    Ps,
    #[strum(to_string = "stop", serialize = "clean")]
    Stop,
    Clear,
    Personality,
    Realtime,
    Settings,
    TestApproval,
    #[strum(serialize = "subagents")]
    MultiAgents,
    // Debugging commands.
    #[strum(serialize = "memory-recall")]
    MemoryRecall,
    MemoryRemember,
    MemoryLessons,
    MemoryCrystals,
    MemoryCrystalsCreate,
    MemoryCrystalsAuto,
    MemoryInsights,
    MemoryReflect,
    MemoryActions,
    MemoryMissions,
    MemoryActionCreate,
    MemoryActionUpdate,
    MemoryHandoffs,
    MemoryHandoffGenerate,
    MemoryFrontier,
    MemoryNext,
    #[strum(to_string = "memory-drop", serialize = "debug-m-drop")]
    MemoryDrop,
    #[strum(to_string = "memory-update", serialize = "debug-m-update")]
    MemoryUpdate,
}

impl SlashCommand {
    /// User-visible description shown in the popup.
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Feedback => "send logs to maintainers",
            SlashCommand::New => "start a new chat during a conversation",
            SlashCommand::Init => "create an AGENTS.md file with instructions for Codex",
            SlashCommand::Compact => "summarize conversation to prevent hitting the context limit",
            SlashCommand::Review => "review my current changes and find issues",
            SlashCommand::Rename => "rename the current thread",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::Clear => "clear the terminal and start a new chat",
            SlashCommand::Fork => "fork the current chat",
            // SlashCommand::Undo => "ask Codex to undo a turn",
            SlashCommand::Quit | SlashCommand::Exit => "exit Codex",
            SlashCommand::Copy => "copy last response as markdown",
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::Mention => "mention a file",
            SlashCommand::Skills => "use skills to improve how Codex performs specific tasks",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::DebugConfig => "show config layers and requirement sources for debugging",
            SlashCommand::Title => "configure which items appear in the terminal title",
            SlashCommand::Statusline => "configure which items appear in the status line",
            SlashCommand::Theme => "choose a syntax highlighting theme",
            SlashCommand::Ps => "list background terminals",
            SlashCommand::Stop => "stop all background terminals",
            SlashCommand::MemoryRecall => "recall relevant memory into the current thread",
            SlashCommand::MemoryRemember => "save durable memory explicitly",
            SlashCommand::MemoryLessons => "review agentmemory lessons",
            SlashCommand::MemoryCrystals => "review crystallized action digests",
            SlashCommand::MemoryCrystalsCreate => "create a crystal from action ids",
            SlashCommand::MemoryCrystalsAuto => "auto-crystallize eligible action groups",
            SlashCommand::MemoryInsights => "review reflected insights",
            SlashCommand::MemoryReflect => "generate reflected insights",
            SlashCommand::MemoryActions => "review tracked action work items",
            SlashCommand::MemoryMissions => "review tracked mission containers",
            SlashCommand::MemoryActionCreate => "create a tracked action work item",
            SlashCommand::MemoryActionUpdate => "update a tracked action work item",
            SlashCommand::MemoryHandoffs => "review durable handoff packets",
            SlashCommand::MemoryHandoffGenerate => "generate a fresh handoff packet",
            SlashCommand::MemoryFrontier => "review unblocked frontier suggestions",
            SlashCommand::MemoryNext => "review the next suggested action",
            SlashCommand::MemoryDrop => "clear stored memories for this workspace",
            SlashCommand::MemoryUpdate => "refresh stored memories for this workspace",
            SlashCommand::Model => "choose what model and reasoning effort to use",
            SlashCommand::Fast => "toggle Fast mode to enable fastest inference at 2X plan usage",
            SlashCommand::Personality => "choose a communication style for Codex",
            SlashCommand::Realtime => "toggle realtime voice mode (experimental)",
            SlashCommand::Settings => "configure realtime microphone/speaker",
            SlashCommand::Plan => "switch to Plan mode",
            SlashCommand::Collab => "change collaboration mode (experimental)",
            SlashCommand::Agent | SlashCommand::MultiAgents => "switch the active agent thread",
            SlashCommand::Approvals => "choose what Codex is allowed to do",
            SlashCommand::Permissions => "choose what Codex is allowed to do",
            SlashCommand::ElevateSandbox => "set up elevated agent sandbox",
            SlashCommand::SandboxReadRoot => {
                "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>"
            }
            SlashCommand::Experimental => "toggle experimental features",
            SlashCommand::Mcp => "list configured MCP tools",
            SlashCommand::Apps => "manage apps",
            SlashCommand::Plugins => "browse plugins",
            SlashCommand::Logout => "log out of Codex",
            SlashCommand::Rollout => "print the rollout file path",
            SlashCommand::TestApproval => "test approval request",
        }
    }

    /// Command string without the leading '/'. Provided for compatibility with
    /// existing code that expects a method named `command()`.
    pub fn command(self) -> &'static str {
        self.into()
    }

    /// Whether this command supports inline args (for example `/review ...`).
    pub fn supports_inline_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Review
                | SlashCommand::Rename
                | SlashCommand::Plan
                | SlashCommand::Fast
                | SlashCommand::Resume
                | SlashCommand::SandboxReadRoot
                | SlashCommand::MemoryRecall
                | SlashCommand::MemoryRemember
                | SlashCommand::MemoryLessons
                | SlashCommand::MemoryCrystalsCreate
                | SlashCommand::MemoryCrystalsAuto
                | SlashCommand::MemoryInsights
                | SlashCommand::MemoryReflect
                | SlashCommand::MemoryActions
                | SlashCommand::MemoryMissions
                | SlashCommand::MemoryActionCreate
                | SlashCommand::MemoryActionUpdate
                | SlashCommand::MemoryHandoffs
                | SlashCommand::MemoryHandoffGenerate
                | SlashCommand::MemoryFrontier
        )
    }

    /// Whether this command can be run while a task is in progress.
    pub fn available_during_task(self) -> bool {
        match self {
            SlashCommand::New
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            // | SlashCommand::Undo
            | SlashCommand::Model
            | SlashCommand::Fast
            | SlashCommand::Personality
            | SlashCommand::Approvals
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::Review
            | SlashCommand::Plan
            | SlashCommand::Clear
            | SlashCommand::Logout
            | SlashCommand::MemoryRecall
            | SlashCommand::MemoryRemember
            | SlashCommand::MemoryLessons
            | SlashCommand::MemoryCrystals
            | SlashCommand::MemoryCrystalsCreate
            | SlashCommand::MemoryCrystalsAuto
            | SlashCommand::MemoryInsights
            | SlashCommand::MemoryReflect
            | SlashCommand::MemoryActions
            | SlashCommand::MemoryMissions
            | SlashCommand::MemoryActionCreate
            | SlashCommand::MemoryActionUpdate
            | SlashCommand::MemoryHandoffs
            | SlashCommand::MemoryHandoffGenerate
            | SlashCommand::MemoryFrontier
            | SlashCommand::MemoryNext
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate => false,
            SlashCommand::Diff
            | SlashCommand::Copy
            | SlashCommand::Rename
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Status
            | SlashCommand::DebugConfig
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Plugins
            | SlashCommand::Feedback
            | SlashCommand::Quit
            | SlashCommand::Exit => true,
            SlashCommand::Rollout => true,
            SlashCommand::TestApproval => true,
            SlashCommand::Realtime => true,
            SlashCommand::Settings => true,
            SlashCommand::Collab => true,
            SlashCommand::Agent | SlashCommand::MultiAgents => true,
            SlashCommand::Statusline => false,
            SlashCommand::Theme => false,
            SlashCommand::Title => false,
        }
    }

    fn is_visible(self) -> bool {
        match self {
            SlashCommand::SandboxReadRoot => cfg!(target_os = "windows"),
            SlashCommand::Copy => !cfg!(target_os = "android"),
            SlashCommand::Rollout | SlashCommand::TestApproval => cfg!(debug_assertions),
            _ => true,
        }
    }
}

/// Return all built-in commands in a Vec paired with their command string.
pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    SlashCommand::iter()
        .filter(|command| command.is_visible())
        .map(|c| (c.command(), c))
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    use super::SlashCommand;

    #[test]
    fn stop_command_is_canonical_name() {
        assert_eq!(SlashCommand::Stop.command(), "stop");
    }

    #[test]
    fn clean_alias_parses_to_stop_command() {
        assert_eq!(SlashCommand::from_str("clean"), Ok(SlashCommand::Stop));
    }
}
