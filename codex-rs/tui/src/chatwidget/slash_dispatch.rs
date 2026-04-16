//! Slash-command dispatch and local-recall handoff for `ChatWidget`.
//!
//! `ChatComposer` parses slash input and stages recognized command text for local
//! Up-arrow recall before returning an input result. This module owns the app-level
//! dispatch step and records the staged entry once the command has been handled, so
//! slash-command recall follows the same submitted-input rule as ordinary text.

use super::*;

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

fn parse_memory_action_update_args(input: &str) -> Option<(String, String)> {
    let mut parts = input.split_whitespace();
    let action_id = parts.next()?.trim();
    let status = parts.next()?.trim().to_ascii_lowercase();
    if action_id.is_empty() || parts.next().is_some() || !is_valid_action_status(status.as_str()) {
        return None;
    }
    Some((action_id.to_string(), status))
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

    pub(super) fn dispatch_command(&mut self, cmd: SlashCommand) {
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
                let init_target = self.config.cwd.join(DEFAULT_PROJECT_DOC_FILENAME);
                if init_target.exists() {
                    let message = format!(
                        "{DEFAULT_PROJECT_DOC_FILENAME} already exists here. Skipping /init to avoid overwriting it."
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
                let next_tier = if matches!(self.config.service_tier, Some(ServiceTier::Fast)) {
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
            SlashCommand::Agent | SlashCommand::MultiAgents => {
                self.app_event_tx.send(AppEvent::OpenAgentPicker);
            }
            SlashCommand::Approvals => {
                self.open_permissions_popup();
            }
            SlashCommand::Permissions => {
                self.open_permissions_popup();
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
            SlashCommand::Quit | SlashCommand::Exit => {
                self.request_quit_without_confirmation();
            }
            SlashCommand::Logout => {
                if let Err(e) = codex_login::logout(
                    &self.config.codex_home,
                    self.config.cli_auth_credentials_store_mode,
                ) {
                    tracing::error!("failed to logout: {e}");
                }
                self.request_quit_without_confirmation();
            }
            // SlashCommand::Undo => {
            //     self.app_event_tx.send(AppEvent::CodexOp(Op::Undo));
            // }
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
            SlashCommand::MemoryActionCreate => {
                self.add_error_message("Usage: /memory-action-create <title>".to_string());
            }
            SlashCommand::MemoryActionUpdate => {
                self.add_error_message(
                    "Usage: /memory-action-update <action_id> <pending|active|done|blocked|cancelled>"
                        .to_string(),
                );
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
                self.add_mcp_output();
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

                use codex_protocol::protocol::ApplyPatchApprovalRequestEvent;
                use codex_protocol::protocol::FileChange;

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
        _text_elements: Vec<TextElement>,
    ) {
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
        match cmd {
            SlashCommand::Fast => {
                if trimmed.is_empty() {
                    self.dispatch_command(cmd);
                    return;
                }
                let prepared_args = if self.bottom_pane.composer_text().is_empty() {
                    args
                } else {
                    let Some((prepared_args, _prepared_elements)) = self
                        .bottom_pane
                        .prepare_inline_args_submission(/*record_history*/ false)
                    else {
                        return;
                    };
                    prepared_args
                };
                match prepared_args.trim().to_ascii_lowercase().as_str() {
                    "on" => self.set_service_tier_selection(Some(ServiceTier::Fast)),
                    "off" => self.set_service_tier_selection(/*service_tier*/ None),
                    "status" => {
                        let status = if matches!(self.config.service_tier, Some(ServiceTier::Fast))
                        {
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
            SlashCommand::Rename if !trimmed.is_empty() => {
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                let Some((prepared_args, _prepared_elements)) = self
                    .bottom_pane
                    .prepare_inline_args_submission(/*record_history*/ false)
                else {
                    return;
                };
                let Some(name) = crate::legacy_core::util::normalize_thread_name(&prepared_args)
                else {
                    self.add_error_message("Thread name cannot be empty.".to_string());
                    return;
                };
                self.app_event_tx.set_thread_name(name);
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::Plan if !trimmed.is_empty() => {
                if !self.apply_plan_slash_command() {
                    return;
                }
                let Some((prepared_args, prepared_elements)) = self
                    .bottom_pane
                    .prepare_inline_args_submission(/*record_history*/ false)
                else {
                    return;
                };
                let local_images = self
                    .bottom_pane
                    .take_recent_submission_images_with_placeholders();
                let remote_image_urls = self.take_remote_image_urls();
                let user_message = UserMessage {
                    text: prepared_args,
                    local_images,
                    remote_image_urls,
                    text_elements: prepared_elements,
                    mention_bindings: self.bottom_pane.take_recent_submission_mention_bindings(),
                };
                if self.is_session_configured() {
                    self.reasoning_buffer.clear();
                    self.full_reasoning_buffer.clear();
                    self.set_status_header(String::from("Working"));
                    self.submit_user_message(user_message);
                } else {
                    self.queue_user_message(user_message);
                }
            }
            SlashCommand::Review if !trimmed.is_empty() => {
                let Some((prepared_args, _prepared_elements)) = self
                    .bottom_pane
                    .prepare_inline_args_submission(/*record_history*/ false)
                else {
                    return;
                };
                self.submit_op(AppCommand::review(ReviewRequest {
                    target: ReviewTarget::Custom {
                        instructions: prepared_args,
                    },
                    user_facing_hint: None,
                }));
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::Resume if !trimmed.is_empty() => {
                let Some((prepared_args, _prepared_elements)) = self
                    .bottom_pane
                    .prepare_inline_args_submission(/*record_history*/ false)
                else {
                    return;
                };
                self.app_event_tx
                    .send(AppEvent::ResumeSessionByIdOrName(prepared_args));
                self.bottom_pane.drain_pending_submission_state();
            }
            SlashCommand::SandboxReadRoot if !trimmed.is_empty() => {
                let Some((prepared_args, _prepared_elements)) = self
                    .bottom_pane
                    .prepare_inline_args_submission(/*record_history*/ false)
                else {
                    return;
                };
                self.app_event_tx
                    .send(AppEvent::BeginWindowsSandboxGrantReadRoot {
                        path: prepared_args,
                    });
                self.bottom_pane.drain_pending_submission_state();
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
    }
}
