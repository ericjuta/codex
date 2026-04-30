//! Slash-command dispatch and local-recall handoff for `ChatWidget`.
//!
//! `ChatComposer` parses slash input and stages recognized command text for local
//! Up-arrow recall before returning an input result. This module owns the app-level
//! dispatch step and records the staged entry once the command has been handled, so
//! slash-command recall follows the same submitted-input rule as ordinary text.

use super::*;
use crate::app_event::ThreadGoalSetMode;
use crate::bottom_pane::prompt_args::parse_slash_name;
use crate::bottom_pane::slash_commands;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlashCommandDispatchSource {
    Live,
    Queued,
}

struct PreparedSlashCommandArgs {
    args: String,
    text_elements: Vec<TextElement>,
    local_images: Vec<LocalImageAttachment>,
    remote_image_urls: Vec<String>,
    mention_bindings: Vec<MentionBinding>,
    source: SlashCommandDispatchSource,
}

const SIDE_STARTING_CONTEXT_LABEL: &str = "Side starting...";
const SIDE_REVIEW_UNAVAILABLE_MESSAGE: &str =
    "'/side' is unavailable while code review is running.";
const SIDE_SLASH_COMMAND_UNAVAILABLE_HINT: &str = "Press Esc to return to the main thread first.";
const GOAL_USAGE: &str = "Usage: /goal <objective>";
const GOAL_USAGE_HINT: &str = "Example: /goal improve benchmark coverage";

fn parse_memory_action_ids(input: &str) -> Option<Vec<String>> {
    let action_ids = input
        .split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!action_ids.is_empty()).then_some(action_ids)
}

fn parse_positive_u32_arg(input: &str) -> Option<u32> {
    input.trim().parse::<u32>().ok().filter(|value| *value > 0)
}

fn is_valid_action_status(status: &str) -> bool {
    matches!(
        status,
        "pending" | "active" | "done" | "blocked" | "cancelled"
    )
}

fn is_valid_mission_status(status: &str) -> bool {
    matches!(
        status,
        "draft" | "active" | "blocked" | "completed" | "cancelled"
    )
}

fn is_valid_handoff_scope_type(scope_type: &str) -> bool {
    matches!(scope_type, "action" | "mission" | "session")
}

fn parse_memory_action_update_args(input: &str) -> Option<(String, String)> {
    let mut parts = input.split_whitespace();
    let action_id = parts.next()?.trim();
    let status = parts.next()?.trim().to_ascii_lowercase();
    if action_id.is_empty() || parts.next().is_some() || !is_valid_action_status(status.as_str()) {
        return None;
    }
    Some((action_id.to_string(), status))
}

fn parse_memory_handoff_generate_args(input: &str) -> Option<(Option<String>, Option<String>)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some((None, None));
    }

    let mut parts = trimmed.split_whitespace();
    let scope_type = parts.next()?.trim().to_ascii_lowercase();
    if !is_valid_handoff_scope_type(scope_type.as_str()) {
        return None;
    }
    let scope_id = parts.next().map(|scope_id| scope_id.trim().to_string());
    if parts.next().is_some() {
        return None;
    }
    if scope_type != "session" && scope_id.is_none() {
        return None;
    }
    Some((Some(scope_type), scope_id))
}

fn parse_memory_handoffs_args(
    input: &str,
) -> Option<(Option<String>, Option<String>, Option<String>)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some((None, None, None));
    }

    let mut parts = trimmed.split_whitespace();
    let first = parts.next()?.trim();
    let normalized = first.to_ascii_lowercase();
    if !is_valid_handoff_scope_type(normalized.as_str()) {
        return parts
            .next()
            .is_none()
            .then_some((Some(trimmed.to_string()), None, None));
    }

    let scope_id = parts.next().map(|scope_id| scope_id.trim().to_string());
    if parts.next().is_some() {
        return None;
    }
    if normalized != "session" && scope_id.is_none() {
        return None;
    }
    Some((None, Some(normalized), scope_id))
}

fn prepared_inline_args(widget: &mut ChatWidget, args: String) -> Option<String> {
    if widget.bottom_pane.composer_text().is_empty() {
        Some(args)
    } else {
        let (prepared_args, _prepared_elements) = widget
            .bottom_pane
            .prepare_inline_args_submission(/*record_history*/ false)?;
        Some(prepared_args)
    }
}

impl ChatWidget {
    /// Dispatch a bare slash command and record its staged local-history entry.
    ///
    /// The composer stages history before returning `InputResult::Command`; this wrapper commits
    /// that staged entry after dispatch so slash-command recall follows the same "submitted input"
    /// rule as normal text.
    pub(super) fn handle_slash_command_dispatch(&mut self, cmd: SlashCommand) {
        self.dispatch_command(cmd);
        if cmd == SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
        self.bottom_pane.record_pending_slash_command_history();
    }

    /// Dispatch an inline slash command and record its staged local-history entry.
    ///
    /// Inline command arguments may later be prepared through the normal submission pipeline, but
    /// local command recall still tracks the original command invocation. Treating this wrapper as
    /// the only input-result entry point avoids double-recording commands with inline args.
    pub(super) fn handle_slash_command_with_args_dispatch(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        self.dispatch_command_with_args(cmd, args, text_elements);
        self.bottom_pane.record_pending_slash_command_history();
    }

    fn apply_plan_slash_command(&mut self) -> bool {
        if !self.collaboration_modes_enabled() {
            self.add_info_message(
                "Collaboration modes are disabled.".to_string(),
                Some("Enable collaboration modes to use /plan.".to_string()),
            );
            return false;
        }
        if let Some(mask) = collaboration_modes::plan_mask(self.model_catalog.as_ref()) {
            self.set_collaboration_mask(mask);
            true
        } else {
            self.add_info_message(
                "Plan mode unavailable right now.".to_string(),
                /*hint*/ None,
            );
            false
        }
    }

    fn request_side_conversation(
        &mut self,
        parent_thread_id: ThreadId,
        user_message: Option<UserMessage>,
    ) {
        self.set_side_conversation_context_label(Some(SIDE_STARTING_CONTEXT_LABEL.to_string()));
        self.request_redraw();
        self.app_event_tx.send(AppEvent::StartSide {
            parent_thread_id,
            user_message,
        });
    }

    fn request_empty_side_conversation(&mut self) {
        let Some(parent_thread_id) = self.thread_id else {
            self.add_error_message("'/side' is unavailable before the session starts.".to_string());
            return;
        };

        self.request_side_conversation(parent_thread_id, /*user_message*/ None);
    }

    pub(super) fn dispatch_command(&mut self, cmd: SlashCommand) {
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.bottom_pane.drain_pending_submission_state();
            self.request_redraw();
            return;
        }

        match cmd {
            SlashCommand::Feedback => {
                if !self.config.feedback_enabled {
                    let params = crate::bottom_pane::feedback_disabled_params();
                    self.bottom_pane.show_selection_view(params);
                    self.request_redraw();
                    return;
                }
                // Step 1: pick a category (UI built in feedback_view)
                let params =
                    crate::bottom_pane::feedback_selection_params(self.app_event_tx.clone());
                self.bottom_pane.show_selection_view(params);
                self.request_redraw();
            }
            SlashCommand::New => {
                self.app_event_tx.send(AppEvent::NewSession);
            }
            SlashCommand::Clear => {
                self.app_event_tx.send(AppEvent::ClearUi);
            }
            SlashCommand::Resume => {
                self.app_event_tx.send(AppEvent::OpenResumePicker);
            }
            SlashCommand::Fork => {
                self.app_event_tx.send(AppEvent::ForkCurrentSession);
            }
            SlashCommand::Init => {
                let init_target = self.config.cwd.join(DEFAULT_AGENTS_MD_FILENAME);
                if init_target.exists() {
                    let message = format!(
                        "{DEFAULT_AGENTS_MD_FILENAME} already exists here. Skipping /init to avoid overwriting it."
                    );
                    self.add_info_message(message, /*hint*/ None);
                    return;
                }
                const INIT_PROMPT: &str = include_str!("../../prompt_for_init_command.md");
                self.submit_user_message(INIT_PROMPT.to_string().into());
            }
            SlashCommand::Compact => {
                self.clear_token_usage();
                if !self.bottom_pane.is_task_running() {
                    self.bottom_pane.set_task_running(/*running*/ true);
                }
                self.app_event_tx.compact();
            }
            SlashCommand::Review => {
                self.open_review_popup();
            }
            SlashCommand::Rename => {
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                self.show_rename_prompt();
            }
            SlashCommand::Model => {
                self.open_model_popup();
            }
            SlashCommand::Fast => {
                let next_tier = if matches!(self.current_service_tier(), Some(ServiceTier::Fast)) {
                    None
                } else {
                    Some(ServiceTier::Fast)
                };
                self.set_service_tier_selection(next_tier);
            }
            SlashCommand::Realtime => {
                if !self.realtime_conversation_enabled() {
                    return;
                }
                if self.realtime_conversation.is_live() {
                    self.stop_realtime_conversation_from_ui();
                } else {
                    self.start_realtime_conversation();
                }
            }
            SlashCommand::Settings => {
                if !self.realtime_audio_device_selection_enabled() {
                    return;
                }
                self.open_realtime_audio_popup();
            }
            SlashCommand::Personality => {
                self.open_personality_popup();
            }
            SlashCommand::Plan => {
                self.apply_plan_slash_command();
            }
            SlashCommand::Goal => {
                if !self.config.features.enabled(Feature::Goals) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadGoalMenu { thread_id });
                } else {
                    self.add_info_message(
                        GOAL_USAGE.to_string(),
                        Some(GOAL_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::Collab => {
                if !self.collaboration_modes_enabled() {
                    self.add_info_message(
                        "Collaboration modes are disabled.".to_string(),
                        Some("Enable collaboration modes to use /collab.".to_string()),
                    );
                    return;
                }
                self.open_collaboration_modes_popup();
            }
            SlashCommand::Side => {
                self.request_empty_side_conversation();
            }
            SlashCommand::Agent | SlashCommand::MultiAgents => {
                self.app_event_tx.send(AppEvent::OpenAgentPicker);
            }
            SlashCommand::Approvals => {
                self.open_permissions_popup();
            }
            SlashCommand::Permissions => {
                self.open_permissions_popup();
            }
            SlashCommand::Vim => {
                self.toggle_vim_mode_and_notify();
            }
            SlashCommand::Keymap => {
                self.open_keymap_picker();
            }
            SlashCommand::ElevateSandbox => {
                #[cfg(target_os = "windows")]
                {
                    let windows_sandbox_level = WindowsSandboxLevel::from_config(&self.config);
                    let windows_degraded_sandbox_enabled =
                        matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken);
                    if !windows_degraded_sandbox_enabled
                        || !crate::legacy_core::windows_sandbox::ELEVATED_SANDBOX_NUX_ENABLED
                    {
                        // This command should not be visible/recognized outside degraded mode,
                        // but guard anyway in case something dispatches it directly.
                        return;
                    }

                    let Some(preset) = builtin_approval_presets()
                        .into_iter()
                        .find(|preset| preset.id == "auto")
                    else {
                        // Avoid panicking in interactive UI; treat this as a recoverable
                        // internal error.
                        self.add_error_message(
                            "Internal error: missing the 'auto' approval preset.".to_string(),
                        );
                        return;
                    };

                    if let Err(err) = self
                        .config
                        .permissions
                        .approval_policy
                        .can_set(&preset.approval)
                    {
                        self.add_error_message(err.to_string());
                        return;
                    }

                    self.session_telemetry.counter(
                        "codex.windows_sandbox.setup_elevated_sandbox_command",
                        /*inc*/ 1,
                        &[],
                    );
                    self.app_event_tx
                        .send(AppEvent::BeginWindowsSandboxElevatedSetup { preset });
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = &self.session_telemetry;
                    // Not supported; on non-Windows this command should never be reachable.
                }
            }
            SlashCommand::SandboxReadRoot => {
                self.add_error_message(
                    "Usage: /sandbox-add-read-dir <absolute-directory-path>".to_string(),
                );
            }
            SlashCommand::Experimental => {
                self.open_experimental_popup();
            }
            SlashCommand::AutoReview => {
                self.open_auto_review_denials_popup();
            }
            SlashCommand::Memories => {
                self.open_memories_popup();
            }
            SlashCommand::Quit | SlashCommand::Exit => {
                self.request_quit_without_confirmation();
            }
            SlashCommand::Logout => {
                self.app_event_tx.send(AppEvent::Logout);
            }
            SlashCommand::Copy => {
                self.copy_last_agent_markdown();
            }
            SlashCommand::Diff => {
                self.add_diff_in_progress();
                let tx = self.app_event_tx.clone();
                tokio::spawn(async move {
                    let text = match get_git_diff().await {
                        Ok((is_git_repo, diff_text)) => {
                            if is_git_repo {
                                diff_text
                            } else {
                                "`/diff` — _not inside a git repository_".to_string()
                            }
                        }
                        Err(e) => format!("Failed to compute diff: {e}"),
                    };
                    tx.send(AppEvent::DiffResult(text));
                });
            }
            SlashCommand::Mention => {
                self.insert_str("@");
            }
            SlashCommand::Skills => {
                self.open_skills_menu();
            }
            SlashCommand::Hooks => {
                self.add_hooks_output();
            }
            SlashCommand::Status => {
                if self.should_prefetch_rate_limits() {
                    let request_id = self.next_status_refresh_request_id;
                    self.next_status_refresh_request_id =
                        self.next_status_refresh_request_id.wrapping_add(1);
                    self.add_status_output(/*refreshing_rate_limits*/ true, Some(request_id));
                    self.app_event_tx.send(AppEvent::RefreshRateLimits {
                        origin: RateLimitRefreshOrigin::StatusCommand { request_id },
                    });
                } else {
                    self.add_status_output(
                        /*refreshing_rate_limits*/ false, /*request_id*/ None,
                    );
                }
            }
            SlashCommand::DebugConfig => {
                self.add_debug_config_output();
            }
            SlashCommand::Title => {
                self.open_terminal_title_setup();
            }
            SlashCommand::Statusline => {
                self.open_status_line_setup();
            }
            SlashCommand::Theme => {
                self.open_theme_picker();
            }
            SlashCommand::Ps => {
                self.add_ps_output();
            }
            SlashCommand::Stop => {
                self.clean_background_terminals();
            }
            SlashCommand::MemoryDrop => {
                self.show_pending_memory_operation(history_cell::new_memory_drop_submission());
                self.submit_op(Op::DropMemories);
            }
            SlashCommand::MemoryUpdate => {
                self.show_pending_memory_operation(history_cell::new_memory_update_submission());
                self.submit_op(Op::UpdateMemories);
            }
            SlashCommand::MemoryRecall => {
                if !self.ensure_memory_recall_thread() {
                    return;
                }
                self.show_pending_memory_operation(history_cell::new_memory_recall_submission(
                    /*query*/ None,
                ));
                self.submit_op(Op::RecallMemories { query: None });
            }
            SlashCommand::MemoryRemember => {
                self.add_error_message("Usage: /memory-remember <content>".to_string());
            }
            SlashCommand::MemoryLessons => {
                self.show_pending_memory_operation(history_cell::new_memory_lessons_submission(
                    None,
                ));
                self.submit_op(Op::ReviewLessons { query: None });
            }
            SlashCommand::MemoryCrystals => {
                self.show_pending_memory_operation(history_cell::new_memory_crystals_submission());
                self.submit_op(Op::ReviewCrystals);
            }
            SlashCommand::MemoryCrystalsCreate => {
                self.add_error_message(
                    "Usage: /memory-crystals-create <action_id[,action_id...]>".to_string(),
                );
            }
            SlashCommand::MemoryCrystalsAuto => {
                self.show_pending_memory_operation(
                    history_cell::new_memory_auto_crystallize_submission(None),
                );
                self.submit_op(Op::AutoCrystallize {
                    older_than_days: None,
                });
            }
            SlashCommand::MemoryInsights => {
                self.show_pending_memory_operation(history_cell::new_memory_insights_submission(
                    None,
                ));
                self.submit_op(Op::ReviewInsights { query: None });
            }
            SlashCommand::MemoryReflect => {
                self.show_pending_memory_operation(history_cell::new_memory_reflect_submission(
                    None,
                ));
                self.submit_op(Op::ReflectMemories { max_clusters: None });
            }
            SlashCommand::MemoryActions => {
                self.show_pending_memory_operation(history_cell::new_memory_actions_submission(
                    None,
                ));
                self.submit_op(Op::ListActions { status: None });
            }
            SlashCommand::MemoryMissions => {
                self.show_pending_memory_operation(history_cell::new_memory_missions_submission(
                    None,
                ));
                self.submit_op(Op::ReviewMissions {
                    mission_id: None,
                    status: None,
                });
            }
            SlashCommand::MemoryBranchOverlays => {
                self.show_pending_memory_operation(
                    history_cell::new_memory_branch_overlays_submission(None),
                );
                self.submit_op(Op::ReviewBranchOverlays { branch: None });
            }
            SlashCommand::MemoryGuardrails => {
                self.show_pending_memory_operation(history_cell::new_memory_guardrails_submission(
                    None,
                ));
                self.submit_op(Op::ReviewGuardrails { query: None });
            }
            SlashCommand::MemoryDecisions => {
                self.show_pending_memory_operation(history_cell::new_memory_decisions_submission(
                    None,
                ));
                self.submit_op(Op::ReviewDecisions { query: None });
            }
            SlashCommand::MemoryDossiers => {
                self.show_pending_memory_operation(history_cell::new_memory_dossiers_submission(
                    None,
                ));
                self.submit_op(Op::ReviewDossiers { file_path: None });
            }
            SlashCommand::MemoryRoutineCandidates => {
                self.show_pending_memory_operation(
                    history_cell::new_memory_routine_candidates_submission(),
                );
                self.submit_op(Op::ReviewRoutineCandidates);
            }
            SlashCommand::MemoryActionCreate => {
                self.add_error_message("Usage: /memory-action-create <title>".to_string());
            }
            SlashCommand::MemoryActionUpdate => {
                self.add_error_message(
                    "Usage: /memory-action-update <action_id> <pending|active|done|blocked|cancelled>"
                        .to_string(),
                );
            }
            SlashCommand::MemoryHandoffs => {
                self.show_pending_memory_operation(history_cell::new_memory_handoffs_submission(
                    None, None,
                ));
                self.submit_op(Op::ReviewHandoffs {
                    handoff_packet_id: None,
                    scope_type: None,
                    scope_id: None,
                });
            }
            SlashCommand::MemoryHandoffGenerate => {
                self.show_pending_memory_operation(
                    history_cell::new_memory_handoff_generate_submission(None, None),
                );
                self.submit_op(Op::GenerateHandoff {
                    scope_type: None,
                    scope_id: None,
                });
            }
            SlashCommand::MemoryFrontier => {
                self.show_pending_memory_operation(history_cell::new_memory_frontier_submission(
                    None,
                ));
                self.submit_op(Op::ReviewFrontier { limit: None });
            }
            SlashCommand::MemoryNext => {
                self.show_pending_memory_operation(history_cell::new_memory_next_submission());
                self.submit_op(Op::ReviewNext);
            }
            SlashCommand::Mcp => {
                self.add_mcp_output(McpServerStatusDetail::ToolsAndAuthOnly);
            }
            SlashCommand::Apps => {
                self.add_connectors_output();
            }
            SlashCommand::Plugins => {
                self.add_plugins_output();
            }
            SlashCommand::Rollout => {
                if let Some(path) = self.rollout_path() {
                    self.add_info_message(
                        format!("Current rollout path: {}", path.display()),
                        /*hint*/ None,
                    );
                } else {
                    self.add_info_message(
                        "Rollout path is not available yet.".to_string(),
                        /*hint*/ None,
                    );
                }
            }
            SlashCommand::TestApproval => {
                use std::collections::HashMap;

                use crate::approval_events::ApplyPatchApprovalRequestEvent;
                use crate::diff_model::FileChange;

                self.on_apply_patch_approval_request(
                    "1".to_string(),
                    ApplyPatchApprovalRequestEvent {
                        call_id: "1".to_string(),
                        turn_id: "turn-1".to_string(),
                        changes: HashMap::from([
                            (
                                PathBuf::from("/tmp/test.txt"),
                                FileChange::Add {
                                    content: "test".to_string(),
                                },
                            ),
                            (
                                PathBuf::from("/tmp/test2.txt"),
                                FileChange::Update {
                                    unified_diff: "+test\n-test2".to_string(),
                                    move_path: None,
                                },
                            ),
                        ]),
                        reason: None,
                        grant_root: Some(PathBuf::from("/tmp")),
                    },
                );
            }
        }
    }

    /// Run an inline slash command.
    ///
    /// Branches that prepare arguments should pass `record_history: false` to the composer because
    /// the staged slash-command entry is the recall record; using the normal submission-history
    /// path as well would make a single command appear twice during Up-arrow navigation.
    pub(super) fn dispatch_command_with_args(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.supports_inline_args() {
            self.dispatch_command(cmd);
            return;
        }
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.request_redraw();
            return;
        }

        let trimmed = args.trim();
        if trimmed.is_empty() {
            self.dispatch_command(cmd);
            return;
        }

        let Some((prepared_args, prepared_elements)) =
            self.prepare_live_inline_args(args, text_elements)
        else {
            return;
        };
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: prepared_args,
                text_elements: prepared_elements,
                local_images: Vec::new(),
                remote_image_urls: Vec::new(),
                mention_bindings: Vec::new(),
                source: SlashCommandDispatchSource::Live,
            },
        );
    }

    fn prepare_live_inline_args(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
    ) -> Option<(String, Vec<TextElement>)> {
        if self.bottom_pane.composer_text().is_empty() {
            Some((args, text_elements))
        } else {
            self.bottom_pane
                .prepare_inline_args_submission(/*record_history*/ false)
        }
    }

    fn prepared_inline_user_message(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
        mut local_images: Vec<LocalImageAttachment>,
        mut remote_image_urls: Vec<String>,
        mut mention_bindings: Vec<MentionBinding>,
        source: SlashCommandDispatchSource,
    ) -> UserMessage {
        if source == SlashCommandDispatchSource::Live {
            local_images = self
                .bottom_pane
                .take_recent_submission_images_with_placeholders();
            remote_image_urls = self.take_remote_image_urls();
            mention_bindings = self.bottom_pane.take_recent_submission_mention_bindings();
        }
        UserMessage {
            text: args,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        }
    }

    fn dispatch_prepared_command_with_args(
        &mut self,
        cmd: SlashCommand,
        prepared: PreparedSlashCommandArgs,
    ) {
        let PreparedSlashCommandArgs {
            args,
            text_elements,
            local_images,
            remote_image_urls,
            mention_bindings,
            source,
        } = prepared;
        let trimmed = args.trim();
        match cmd {
            SlashCommand::Fast => {
                match trimmed.to_ascii_lowercase().as_str() {
                    "on" => self.set_service_tier_selection(Some(ServiceTier::Fast)),
                    "off" => self.set_service_tier_selection(/*service_tier*/ None),
                    "status" => {
                        let status =
                            if matches!(self.current_service_tier(), Some(ServiceTier::Fast)) {
                                "on"
                            } else {
                                "off"
                            };
                        self.add_info_message(
                            format!("Fast mode is {status}."),
                            /*hint*/ None,
                        );
                    }
                    _ => {
                        self.add_error_message("Usage: /fast [on|off|status]".to_string());
                    }
                }
            }
            SlashCommand::Mcp => match trimmed.to_ascii_lowercase().as_str() {
                "verbose" => self.add_mcp_output(McpServerStatusDetail::Full),
                _ => self.add_error_message("Usage: /mcp [verbose]".to_string()),
            },
            SlashCommand::Rename if !trimmed.is_empty() => {
                if !self.ensure_thread_rename_allowed() {
                    return;
                }
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                let Some(name) = crate::legacy_core::util::normalize_thread_name(&args) else {
                    self.add_error_message("Thread name cannot be empty.".to_string());
                    return;
                };
                self.app_event_tx.set_thread_name(name);
            }
            SlashCommand::Plan if !trimmed.is_empty() => {
                if !self.apply_plan_slash_command() {
                    return;
                }
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                if self.is_session_configured() {
                    self.reasoning_buffer.clear();
                    self.full_reasoning_buffer.clear();
                    self.set_status_header(String::from("Working"));
                    self.submit_user_message(user_message);
                } else {
                    self.queue_user_message(user_message);
                }
            }
            SlashCommand::Goal if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::Goals) {
                    return;
                }
                enum GoalControlCommand {
                    Clear,
                    SetStatus(AppThreadGoalStatus),
                }
                let control_command = match trimmed.to_ascii_lowercase().as_str() {
                    "clear" => Some(GoalControlCommand::Clear),
                    "pause" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Paused)),
                    "resume" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Active)),
                    _ => None,
                };
                if let Some(command) = control_command {
                    let Some(thread_id) = self.thread_id else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some(
                                "The session must start before you can change a goal.".to_string(),
                            ),
                        );
                        return;
                    };
                    match command {
                        GoalControlCommand::Clear => {
                            self.app_event_tx
                                .send(AppEvent::ClearThreadGoal { thread_id });
                        }
                        GoalControlCommand::SetStatus(status) => {
                            self.app_event_tx
                                .send(AppEvent::SetThreadGoalStatus { thread_id, status });
                        }
                    }
                    if source == SlashCommandDispatchSource::Live {
                        self.bottom_pane.drain_pending_submission_state();
                    }
                    return;
                }
                let objective = args.trim();
                if objective.is_empty() {
                    self.add_error_message("Goal objective must not be empty.".to_string());
                    self.add_info_message(
                        GOAL_USAGE.to_string(),
                        Some(GOAL_USAGE_HINT.to_string()),
                    );
                    if source == SlashCommandDispatchSource::Live {
                        self.bottom_pane.drain_pending_submission_state();
                    }
                    return;
                }
                let Some(thread_id) = self.thread_id else {
                    if source == SlashCommandDispatchSource::Live {
                        self.queue_user_message_with_options(
                            UserMessage {
                                text: format!("/goal {args}"),
                                local_images: Vec::new(),
                                remote_image_urls: Vec::new(),
                                text_elements: Vec::new(),
                                mention_bindings: Vec::new(),
                            },
                            QueuedInputAction::ParseSlash,
                        );
                        self.bottom_pane.drain_pending_submission_state();
                    } else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some("The session must start before you can set a goal.".to_string()),
                        );
                    }
                    return;
                };
                self.app_event_tx.send(AppEvent::SetThreadGoalObjective {
                    thread_id,
                    objective: objective.to_string(),
                    mode: ThreadGoalSetMode::ConfirmIfExists,
                });
                if source == SlashCommandDispatchSource::Live {
                    self.bottom_pane.drain_pending_submission_state();
                }
            }
            SlashCommand::Side if !trimmed.is_empty() => {
                let Some(parent_thread_id) = self.thread_id else {
                    self.add_error_message(
                        "'/side' is unavailable before the session starts.".to_string(),
                    );
                    return;
                };
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                self.request_side_conversation(parent_thread_id, Some(user_message));
            }
            SlashCommand::Review if !trimmed.is_empty() => {
                self.submit_op(AppCommand::review(ReviewTarget::Custom {
                    instructions: args,
                }));
            }
            SlashCommand::Resume if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::ResumeSessionByIdOrName(args));
            }
            SlashCommand::SandboxReadRoot if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::BeginWindowsSandboxGrantReadRoot { path: args });
            }
            SlashCommand::MemoryRecall if !trimmed.is_empty() => {
                if !self.ensure_memory_recall_thread() {
                    return;
                }
                self.show_pending_memory_operation(history_cell::new_memory_recall_submission(
                    Some(trimmed.to_string()),
                ));
                self.submit_op(Op::RecallMemories {
                    query: Some(trimmed.to_string()),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryRemember if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                self.show_pending_memory_operation(history_cell::new_memory_remember_submission(
                    prepared_args.clone(),
                ));
                self.submit_op(Op::RememberMemories {
                    content: prepared_args,
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryLessons if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                self.show_pending_memory_operation(history_cell::new_memory_lessons_submission(
                    Some(prepared_args.clone()),
                ));
                self.submit_op(Op::ReviewLessons {
                    query: Some(prepared_args),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryCrystalsCreate if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let Some(action_ids) = parse_memory_action_ids(prepared_args.as_str()) else {
                    self.add_error_message(
                        "Usage: /memory-crystals-create <action_id[,action_id...]>".to_string(),
                    );
                    return;
                };
                self.show_pending_memory_operation(
                    history_cell::new_memory_crystallize_submission(action_ids.clone()),
                );
                self.submit_op(Op::CreateCrystals { action_ids });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryCrystalsAuto if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let Some(older_than_days) = parse_positive_u32_arg(prepared_args.as_str()) else {
                    self.add_error_message(
                        "Usage: /memory-crystals-auto [older_than_days]".to_string(),
                    );
                    return;
                };
                self.show_pending_memory_operation(
                    history_cell::new_memory_auto_crystallize_submission(Some(older_than_days)),
                );
                self.submit_op(Op::AutoCrystallize {
                    older_than_days: Some(older_than_days),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryInsights if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                self.show_pending_memory_operation(history_cell::new_memory_insights_submission(
                    Some(prepared_args.clone()),
                ));
                self.submit_op(Op::ReviewInsights {
                    query: Some(prepared_args),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryReflect if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let Some(max_clusters) = parse_positive_u32_arg(prepared_args.as_str()) else {
                    self.add_error_message("Usage: /memory-reflect [max_clusters]".to_string());
                    return;
                };
                self.show_pending_memory_operation(history_cell::new_memory_reflect_submission(
                    Some(max_clusters),
                ));
                self.submit_op(Op::ReflectMemories {
                    max_clusters: Some(max_clusters),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryActions if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let status = prepared_args.trim().to_ascii_lowercase();
                if !is_valid_action_status(status.as_str()) {
                    self.add_error_message(
                        "Usage: /memory-actions [pending|active|done|blocked|cancelled]"
                            .to_string(),
                    );
                    return;
                }
                self.show_pending_memory_operation(history_cell::new_memory_actions_submission(
                    Some(status.clone()),
                ));
                self.submit_op(Op::ListActions {
                    status: Some(status),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryMissions if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let normalized = prepared_args.trim().to_ascii_lowercase();
                let (mission_id, status) = if is_valid_mission_status(normalized.as_str()) {
                    (None, Some(normalized))
                } else {
                    (Some(prepared_args), None)
                };
                self.show_pending_memory_operation(history_cell::new_memory_missions_submission(
                    mission_id.clone().or(status.clone()),
                ));
                self.submit_op(Op::ReviewMissions { mission_id, status });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryBranchOverlays if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                self.show_pending_memory_operation(
                    history_cell::new_memory_branch_overlays_submission(Some(
                        prepared_args.clone(),
                    )),
                );
                self.submit_op(Op::ReviewBranchOverlays {
                    branch: Some(prepared_args),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryGuardrails if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                self.show_pending_memory_operation(history_cell::new_memory_guardrails_submission(
                    Some(prepared_args.clone()),
                ));
                self.submit_op(Op::ReviewGuardrails {
                    query: Some(prepared_args),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryDecisions if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                self.show_pending_memory_operation(history_cell::new_memory_decisions_submission(
                    Some(prepared_args.clone()),
                ));
                self.submit_op(Op::ReviewDecisions {
                    query: Some(prepared_args),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryDossiers if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                self.show_pending_memory_operation(history_cell::new_memory_dossiers_submission(
                    Some(prepared_args.clone()),
                ));
                self.submit_op(Op::ReviewDossiers {
                    file_path: Some(prepared_args),
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryActionCreate if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                self.show_pending_memory_operation(
                    history_cell::new_memory_action_create_submission(prepared_args.clone()),
                );
                self.submit_op(Op::CreateAction {
                    title: prepared_args,
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryActionUpdate if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let Some((action_id, status)) =
                    parse_memory_action_update_args(prepared_args.as_str())
                else {
                    self.add_error_message(
                        "Usage: /memory-action-update <action_id> <pending|active|done|blocked|cancelled>"
                            .to_string(),
                    );
                    return;
                };
                self.show_pending_memory_operation(
                    history_cell::new_memory_action_update_submission(
                        action_id.clone(),
                        status.clone(),
                    ),
                );
                self.submit_op(Op::UpdateAction { action_id, status });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryHandoffs if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let Some((handoff_packet_id, scope_type, mut scope_id)) =
                    parse_memory_handoffs_args(prepared_args.as_str())
                else {
                    self.add_error_message(
                        "Usage: /memory-handoffs [<handoff_packet_id> | session [scope_id] | mission <mission_id> | action <action_id>]".to_string(),
                    );
                    return;
                };
                if scope_type.as_deref() == Some("session") && scope_id.is_none() {
                    scope_id = self.thread_id().map(|thread_id| thread_id.to_string());
                }
                let query = handoff_packet_id.clone().or_else(|| {
                    scope_type
                        .clone()
                        .zip(scope_id.clone())
                        .map(|(scope_type, scope_id)| format!("{scope_type} {scope_id}"))
                });
                self.show_pending_memory_operation(history_cell::new_memory_handoffs_submission(
                    query,
                    scope_id.clone(),
                ));
                self.submit_op(Op::ReviewHandoffs {
                    handoff_packet_id,
                    scope_type,
                    scope_id,
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryHandoffGenerate if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let Some((scope_type, scope_id)) =
                    parse_memory_handoff_generate_args(prepared_args.as_str())
                else {
                    self.add_error_message(
                        "Usage: /memory-handoff-generate [session [scope_id] | mission <mission_id> | action <action_id>]".to_string(),
                    );
                    return;
                };
                self.show_pending_memory_operation(
                    history_cell::new_memory_handoff_generate_submission(
                        scope_type.clone(),
                        scope_id.clone(),
                    ),
                );
                self.submit_op(Op::GenerateHandoff {
                    scope_type,
                    scope_id,
                });
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::MemoryFrontier if !trimmed.is_empty() => {
                let Some(prepared_args) = prepared_inline_args(self, args) else {
                    return;
                };
                let Some(limit) = parse_positive_u32_arg(prepared_args.as_str()) else {
                    self.add_error_message("Usage: /memory-frontier [limit]".to_string());
                    return;
                };
                self.show_pending_memory_operation(history_cell::new_memory_frontier_submission(
                    Some(limit),
                ));
                self.submit_op(Op::ReviewFrontier { limit: Some(limit) });
                self.bottom_pane.drain_pending_submission_state();
            }
            _ => self.dispatch_command(cmd),
        }
        if source == SlashCommandDispatchSource::Live && cmd != SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    pub(super) fn submit_queued_slash_prompt(&mut self, user_message: UserMessage) -> QueueDrain {
        let UserMessage {
            text,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        } = user_message;
        let Some((name, rest, rest_offset)) = parse_slash_name(&text) else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        if name.contains('/') {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }

        let Some(cmd) = slash_commands::find_builtin_command(name, self.builtin_command_flags())
        else {
            self.add_info_message(
                format!(
                    r#"Unrecognized command '/{name}'. Type "/" for a list of supported commands."#
                ),
                /*hint*/ None,
            );
            return QueueDrain::Continue;
        };

        if rest.is_empty() {
            self.dispatch_command(cmd);
            return self.queued_command_drain_result(cmd);
        }

        if !cmd.supports_inline_args() {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }

        let trimmed_start = rest.trim_start();
        let leading_trimmed = rest.len().saturating_sub(trimmed_start.len());
        let trimmed_rest = trimmed_start.trim_end();
        let args_elements = Self::slash_command_args_elements(
            trimmed_rest,
            rest_offset + leading_trimmed,
            &text_elements,
        );
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: trimmed_rest.to_string(),
                text_elements: args_elements,
                local_images,
                remote_image_urls,
                mention_bindings,
                source: SlashCommandDispatchSource::Queued,
            },
        );
        self.queued_command_drain_result(cmd)
    }

    fn builtin_command_flags(&self) -> slash_commands::BuiltinCommandFlags {
        #[cfg(target_os = "windows")]
        let allow_elevate_sandbox = {
            let windows_sandbox_level = WindowsSandboxLevel::from_config(&self.config);
            matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken)
        };
        #[cfg(not(target_os = "windows"))]
        let allow_elevate_sandbox = false;

        slash_commands::BuiltinCommandFlags {
            collaboration_modes_enabled: self.collaboration_modes_enabled(),
            connectors_enabled: self.connectors_enabled(),
            plugins_command_enabled: self.config.features.enabled(Feature::Plugins),
            goal_command_enabled: self.config.features.enabled(Feature::Goals),
            fast_command_enabled: self.fast_mode_enabled(),
            personality_command_enabled: self.config.features.enabled(Feature::Personality),
            realtime_conversation_enabled: self.realtime_conversation_enabled(),
            audio_device_selection_enabled: self.realtime_audio_device_selection_enabled(),
            allow_elevate_sandbox,
            side_conversation_active: self.active_side_conversation,
        }
    }

    fn queued_command_drain_result(&self, cmd: SlashCommand) -> QueueDrain {
        if self.is_user_turn_pending_or_running() || !self.bottom_pane.no_modal_or_popup_active() {
            return QueueDrain::Stop;
        }
        match cmd {
            SlashCommand::Fast
            | SlashCommand::Status
            | SlashCommand::DebugConfig
            | SlashCommand::Ps
            | SlashCommand::Stop
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
            | SlashCommand::MemoryBranchOverlays
            | SlashCommand::MemoryGuardrails
            | SlashCommand::MemoryDecisions
            | SlashCommand::MemoryDossiers
            | SlashCommand::MemoryRoutineCandidates
            | SlashCommand::MemoryActionCreate
            | SlashCommand::MemoryActionUpdate
            | SlashCommand::MemoryHandoffs
            | SlashCommand::MemoryHandoffGenerate
            | SlashCommand::MemoryFrontier
            | SlashCommand::MemoryNext
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Plugins
            | SlashCommand::Rollout
            | SlashCommand::Copy
            | SlashCommand::Vim
            | SlashCommand::Diff
            | SlashCommand::Rename
            | SlashCommand::TestApproval => QueueDrain::Continue,
            SlashCommand::Feedback
            | SlashCommand::New
            | SlashCommand::Clear
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Review
            | SlashCommand::Model
            | SlashCommand::Realtime
            | SlashCommand::Settings
            | SlashCommand::Personality
            | SlashCommand::Plan
            | SlashCommand::Goal
            | SlashCommand::Collab
            | SlashCommand::Side
            | SlashCommand::Keymap
            | SlashCommand::Agent
            | SlashCommand::MultiAgents
            | SlashCommand::Approvals
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::AutoReview
            | SlashCommand::Memories
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Logout
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Hooks
            | SlashCommand::Title
            | SlashCommand::Statusline
            | SlashCommand::Theme => QueueDrain::Stop,
        }
    }

    fn slash_command_args_elements(
        rest: &str,
        rest_offset: usize,
        text_elements: &[TextElement],
    ) -> Vec<TextElement> {
        if rest.is_empty() || text_elements.is_empty() {
            return Vec::new();
        }
        text_elements
            .iter()
            .filter_map(|elem| {
                if elem.byte_range.end <= rest_offset {
                    return None;
                }
                let start = elem.byte_range.start.saturating_sub(rest_offset);
                let mut end = elem.byte_range.end.saturating_sub(rest_offset);
                if start >= rest.len() {
                    return None;
                }
                end = end.min(rest.len());
                (start < end).then_some(elem.map_range(|_| ByteRange { start, end }))
            })
            .collect()
    }

    fn ensure_slash_command_allowed_in_side_conversation(&mut self, cmd: SlashCommand) -> bool {
        if !self.active_side_conversation || cmd.available_in_side_conversation() {
            return true;
        }
        self.add_error_message(format!(
            "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
            cmd.command()
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }

    fn ensure_side_command_allowed_outside_review(&mut self, cmd: SlashCommand) -> bool {
        if cmd != SlashCommand::Side || !self.is_review_mode {
            return true;
        }

        self.add_error_message(SIDE_REVIEW_UNAVAILABLE_MESSAGE.to_string());
        self.bottom_pane.drain_pending_submission_state();
        false
    }
}
