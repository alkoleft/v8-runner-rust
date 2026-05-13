use super::*;

pub(super) fn run_build_designer(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    debug!(
        full_rebuild = args.full_rebuild,
        source_set = args.source_set.as_deref(),
        "preparing build plan"
    );

    let started = Instant::now();
    let inventory = SourceSetInventory::new(config);
    let ordered_source_sets =
        match selected_ordered_source_sets(&inventory, args.source_set.as_deref()) {
            Ok(source_sets) => source_sets,
            Err(error) => {
                return Err(BuildExecutionFailure::with_payload(
                    error,
                    BuildResult {
                        ok: false,
                        steps: vec![],
                        duration_ms: started.elapsed().as_millis() as u64,
                    },
                ));
            }
        };
    let selected_designer_contexts =
        designer_contexts_for_source_sets(&inventory, &ordered_source_sets);

    let analysis_by_name = if args.full_rebuild {
        None
    } else {
        Some(analyze_contexts_by_name(
            &inventory,
            &selected_designer_contexts,
        ))
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let mut designer_binary: Option<PathBuf> = None;
    let mut steps = Vec::new();

    for (index, source_set) in ordered_source_sets.iter().enumerate() {
        let Some(source_context) = inventory.designer_context(&source_set.name).cloned() else {
            continue;
        };

        if source_set.purpose.is_external() {
            let step_started = Instant::now();
            let result = discover_designer_external_artifacts(
                &source_set.name,
                &inventory.source_path(source_set),
                source_set_external_kind(source_set).expect("external kind"),
            );
            match result {
                Ok(descriptors) => push_build_step(
                    &mut steps,
                    &source_set.name,
                    BuildMode::Skipped,
                    true,
                    format!(
                        "prepared {} external artifact(s) for packaging",
                        descriptors.len()
                    ),
                    step_started.elapsed().as_millis() as u64,
                ),
                Err(error) => {
                    let result = fail_from_source_set_index(
                        started,
                        steps,
                        &ordered_source_sets,
                        index,
                        source_set,
                        BuildMode::Skipped,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(error, result));
                }
            }
            continue;
        }

        let plan = match plan_configurator_load_step(
            source_set,
            &source_context,
            args.full_rebuild,
            analysis_by_name.as_ref(),
            config.build.partial_load_threshold,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let result = fail_from_source_set_index(
                    started,
                    steps,
                    &ordered_source_sets,
                    index,
                    source_set,
                    BuildMode::Skipped,
                    error.to_string(),
                );
                return Err(BuildExecutionFailure::with_payload(
                    AppError::Runtime(error.to_string()),
                    result,
                ));
            }
        };

        match plan {
            StepPlan::Skip { message, ok } => {
                debug!(
                    source_set = source_set.name.as_str(),
                    message = message.as_str(),
                    "skipping build step"
                );
                push_build_step(
                    &mut steps,
                    &source_set.name,
                    BuildMode::Skipped,
                    ok,
                    message,
                    0,
                )
            }
            StepPlan::Execute {
                mode,
                message,
                partial_paths,
                commit,
            } => {
                debug!(
                    source_set = source_set.name.as_str(),
                    mode = ?mode,
                    message = message.as_str(),
                    "executing build step"
                );
                let binary = match designer_binary.clone() {
                    Some(path) => path,
                    None => {
                        let location = match utilities.locate(UtilityType::V8) {
                            Ok(location) => location,
                            Err(error) => {
                                let result = fail_from_source_set_index(
                                    started,
                                    steps,
                                    &ordered_source_sets,
                                    index,
                                    source_set,
                                    mode.clone(),
                                    error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(
                                    AppError::from(error),
                                    result,
                                ));
                            }
                        };
                        designer_binary = Some(location.path.clone());
                        location.path
                    }
                };

                let step_started = Instant::now();
                match execute_source_set_step(
                    context,
                    config,
                    &binary,
                    utilities.runner_for(UtilityType::V8),
                    source_set,
                    &source_context,
                    &source_context,
                    index,
                    partial_paths.as_deref(),
                    &commit,
                    resolve_dynamic_update(config, args),
                ) {
                    Ok(warnings) => push_build_step(
                        &mut steps,
                        &source_set.name,
                        mode,
                        true,
                        merge_step_message(message, &warnings),
                        step_started.elapsed().as_millis() as u64,
                    ),
                    Err(error) => {
                        let result = fail_from_source_set_index(
                            started,
                            steps,
                            &ordered_source_sets,
                            index,
                            source_set,
                            mode,
                            error.to_string(),
                        );
                        return Err(BuildExecutionFailure::with_payload(error, result));
                    }
                }
            }
        }
    }

    Ok(BuildResult {
        ok: true,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

pub(super) fn run_build_ibcmd(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    debug!(
        full_rebuild = args.full_rebuild,
        source_set = args.source_set.as_deref(),
        "preparing ibcmd build plan"
    );

    let started = Instant::now();
    let inventory = SourceSetInventory::new(config);
    let ordered_source_sets =
        match selected_ordered_source_sets(&inventory, args.source_set.as_deref()) {
            Ok(source_sets) => source_sets,
            Err(error) => {
                return Err(BuildExecutionFailure::with_payload(
                    error,
                    BuildResult {
                        ok: false,
                        steps: vec![],
                        duration_ms: started.elapsed().as_millis() as u64,
                    },
                ));
            }
        };
    let selected_designer_contexts =
        designer_contexts_for_source_sets(&inventory, &ordered_source_sets);

    let analysis_by_name = if args.full_rebuild {
        None
    } else {
        Some(analyze_contexts_by_name(
            &inventory,
            &selected_designer_contexts,
        ))
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let mut ibcmd_binary: Option<PathBuf> = None;
    let mut steps = Vec::new();

    for (index, source_set) in ordered_source_sets.iter().enumerate() {
        let Some(source_context) = inventory.designer_context(&source_set.name).cloned() else {
            continue;
        };

        let plan = match plan_configurator_load_step(
            source_set,
            &source_context,
            args.full_rebuild,
            analysis_by_name.as_ref(),
            config.build.partial_load_threshold,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let result = fail_from_source_set_index(
                    started,
                    steps,
                    &ordered_source_sets,
                    index,
                    source_set,
                    BuildMode::Skipped,
                    error.to_string(),
                );
                return Err(BuildExecutionFailure::with_payload(
                    AppError::Runtime(error.to_string()),
                    result,
                ));
            }
        };

        match plan {
            StepPlan::Skip { message, ok } => {
                debug!(
                    source_set = source_set.name.as_str(),
                    message = message.as_str(),
                    "skipping build step"
                );
                push_build_step(
                    &mut steps,
                    &source_set.name,
                    BuildMode::Skipped,
                    ok,
                    message,
                    0,
                )
            }
            StepPlan::Execute {
                mode,
                message,
                partial_paths,
                commit,
            } => {
                debug!(
                    source_set = source_set.name.as_str(),
                    mode = ?mode,
                    message = message.as_str(),
                    "executing ibcmd build step"
                );
                let binary = match ibcmd_binary.clone() {
                    Some(path) => path,
                    None => {
                        let location = match utilities.locate(UtilityType::Ibcmd) {
                            Ok(location) => location,
                            Err(error) => {
                                let result = fail_from_source_set_index(
                                    started,
                                    steps,
                                    &ordered_source_sets,
                                    index,
                                    source_set,
                                    mode.clone(),
                                    error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(
                                    AppError::from(error),
                                    result,
                                ));
                            }
                        };
                        ibcmd_binary = Some(location.path.clone());
                        location.path
                    }
                };

                let step_started = Instant::now();
                match execute_source_set_step_ibcmd(
                    context,
                    config,
                    &binary,
                    utilities.runner_for(UtilityType::Ibcmd),
                    source_set,
                    &source_context,
                    &source_context,
                    partial_paths.as_deref(),
                    &commit,
                ) {
                    Ok(warnings) => push_build_step(
                        &mut steps,
                        &source_set.name,
                        mode,
                        true,
                        merge_step_message(message, &warnings),
                        step_started.elapsed().as_millis() as u64,
                    ),
                    Err(error) => {
                        let result = fail_from_source_set_index(
                            started,
                            steps,
                            &ordered_source_sets,
                            index,
                            source_set,
                            mode,
                            error.to_string(),
                        );
                        return Err(BuildExecutionFailure::with_payload(error, result));
                    }
                }
            }
        }
    }

    Ok(BuildResult {
        ok: true,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

pub(super) fn run_build_edt(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &BuildArgs,
) -> Result<BuildResult, BuildExecutionFailure> {
    debug!(
        full_rebuild = args.full_rebuild,
        source_set = args.source_set.as_deref(),
        "preparing edt build plan"
    );
    if let Some(error) = validate_edt_supported_matrix(config) {
        return Err(BuildExecutionFailure::with_payload(
            error,
            BuildResult {
                ok: false,
                steps: vec![],
                duration_ms: 0,
            },
        ));
    }

    let started = Instant::now();
    let inventory = SourceSetInventory::new(config);
    let ordered_source_sets =
        match selected_ordered_source_sets(&inventory, args.source_set.as_deref()) {
            Ok(source_sets) => source_sets,
            Err(error) => {
                return Err(BuildExecutionFailure::with_payload(
                    error,
                    BuildResult {
                        ok: false,
                        steps: vec![],
                        duration_ms: started.elapsed().as_millis() as u64,
                    },
                ));
            }
        };
    let selected_edt_contexts = edt_contexts_for_source_sets(&inventory, &ordered_source_sets);

    let edt_analysis_by_name = if args.full_rebuild {
        None
    } else {
        Some(analyze_contexts_by_name(&inventory, &selected_edt_contexts))
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let mut designer_binary: Option<PathBuf> = None;
    let mut ibcmd_binary: Option<PathBuf> = None;
    let mut edt_binary: Option<PathBuf> = None;
    let mut interactive_edt = None;
    let mut steps = Vec::new();

    for (index, source_set) in ordered_source_sets.iter().enumerate() {
        let Some(edt_context) = inventory.edt_context(&source_set.name).cloned() else {
            continue;
        };
        let Some(designer_context) = inventory.designer_context(&source_set.name).cloned() else {
            continue;
        };

        let edt_stage = match plan_edt_export_step(
            source_set,
            args.full_rebuild,
            edt_analysis_by_name.as_ref(),
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let result = fail_from_source_set_index(
                    started,
                    steps,
                    &ordered_source_sets,
                    index,
                    source_set,
                    BuildMode::Skipped,
                    error.to_string(),
                );
                return Err(BuildExecutionFailure::with_payload(
                    AppError::Runtime(error.to_string()),
                    result,
                ));
            }
        };

        if source_set.purpose.is_external() {
            let edt = match edt_binary.clone() {
                Some(path) => path,
                None => {
                    let location = match utilities.locate(UtilityType::EdtCli) {
                        Ok(location) => location,
                        Err(error) => {
                            let result = fail_from_source_set_index(
                                started,
                                steps,
                                &ordered_source_sets,
                                index,
                                source_set,
                                BuildMode::EdtExport,
                                error.to_string(),
                            );
                            return Err(BuildExecutionFailure::with_payload(
                                AppError::from(error),
                                result,
                            ));
                        }
                    };
                    edt_binary = Some(location.path.clone());
                    location.path
                }
            };
            let export_started = Instant::now();
            if let Some(error) = interruption_before_safe_point(
                context,
                format!(
                    "EDT external artifact export for source-set '{}'",
                    source_set.name
                ),
            ) {
                let result = fail_from_source_set_index(
                    started,
                    steps,
                    &ordered_source_sets,
                    index,
                    source_set,
                    BuildMode::EdtExport,
                    error.to_string(),
                );
                return Err(BuildExecutionFailure::with_payload(error, result));
            }
            log_timeline_stage(
                &source_set.name,
                "edt_export",
                "[EDT] Конвертация внешних объектов в файлы конфигуратора",
                TimelineStageStatus::Running,
            );
            let export_result = if config.tools.edt_cli.interactive_mode {
                if interactive_edt.is_none() {
                    interactive_edt = Some(
                        match EdtSessionManager::for_config(
                            config,
                            EdtSessionHostOptions::for_cli_command(config),
                        ) {
                            Ok(manager) => match EdtDsl::new_shared_session(
                                edt.clone(),
                                config.work_path.join("edt-workspace"),
                                Arc::new(manager),
                                Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
                                Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
                            ) {
                                Ok(dsl) => dsl.with_execution_policy(context.process_policy(
                                    InterruptionSafetyClass::GracefulThenKill,
                                    None,
                                )),
                                Err(error) => {
                                    let app_error = AppError::from(error);
                                    let result = fail_from_source_set_index(
                                        started,
                                        steps,
                                        &ordered_source_sets,
                                        index,
                                        source_set,
                                        BuildMode::EdtExport,
                                        app_error.to_string(),
                                    );
                                    return Err(BuildExecutionFailure::with_payload(
                                        app_error, result,
                                    ));
                                }
                            },
                            Err(error) => {
                                let app_error = AppError::from(error);
                                let result = fail_from_source_set_index(
                                    started,
                                    steps,
                                    &ordered_source_sets,
                                    index,
                                    source_set,
                                    BuildMode::EdtExport,
                                    app_error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(app_error, result));
                            }
                        },
                    );
                }
                prepare_edt_external_artifacts(
                    config,
                    source_set,
                    interactive_edt.as_ref().expect("interactive edt dsl"),
                )
            } else {
                let one_shot_edt = EdtDsl::new(
                    edt.clone(),
                    config.work_path.join("edt-workspace"),
                    utilities.runner_for(UtilityType::EdtCli),
                )
                .with_execution_policy(
                    context.process_policy(InterruptionSafetyClass::GracefulThenKill, None),
                );
                prepare_edt_external_artifacts(config, source_set, &one_shot_edt)
            };
            match export_result {
                Ok(descriptors) => {
                    match &edt_stage {
                        StepPlan::Execute { commit, .. } => {
                            if let Err(app_error) = commit_step_state(
                                source_set,
                                &edt_context,
                                &config.work_path,
                                commit,
                            ) {
                                let result = fail_from_source_set_index(
                                    started,
                                    steps,
                                    &ordered_source_sets,
                                    index,
                                    source_set,
                                    BuildMode::EdtExport,
                                    app_error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(app_error, result));
                            }
                        }
                        StepPlan::Skip { .. } => {}
                    }
                    push_build_step(
                        &mut steps,
                        &source_set.name,
                        BuildMode::EdtExport,
                        true,
                        merge_step_message(
                            format!(
                                "exported {} external artifact(s) to designer runtime",
                                descriptors.len()
                            ),
                            &[],
                        ),
                        export_started.elapsed().as_millis() as u64,
                    )
                }
                Err(error) => {
                    let result = fail_from_source_set_index(
                        started,
                        steps,
                        &ordered_source_sets,
                        index,
                        source_set,
                        BuildMode::EdtExport,
                        error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(error, result));
                }
            }
            continue;
        }

        let edt_stage_skipped = matches!(&edt_stage, StepPlan::Skip { .. });

        match edt_stage {
            StepPlan::Skip { message, ok } => {
                push_build_step(
                    &mut steps,
                    &source_set.name,
                    BuildMode::Skipped,
                    ok,
                    message,
                    0,
                );
            }
            StepPlan::Execute {
                message: _,
                partial_paths: _,
                commit,
                mode: _,
            } => {
                let edt = match edt_binary.clone() {
                    Some(path) => path,
                    None => {
                        let location = match utilities.locate(UtilityType::EdtCli) {
                            Ok(location) => location,
                            Err(error) => {
                                let result = fail_from_source_set_index(
                                    started,
                                    steps,
                                    &ordered_source_sets,
                                    index,
                                    source_set,
                                    BuildMode::EdtExport,
                                    error.to_string(),
                                );
                                return Err(BuildExecutionFailure::with_payload(
                                    AppError::from(error),
                                    result,
                                ));
                            }
                        };
                        edt_binary = Some(location.path.clone());
                        location.path
                    }
                };

                let export_started = Instant::now();
                log_timeline_stage(
                    &source_set.name,
                    "edt_export",
                    "[EDT] Конвертация в файлы конфигуратора",
                    TimelineStageStatus::Running,
                );
                let export_result = if config.tools.edt_cli.interactive_mode {
                    if interactive_edt.is_none() {
                        interactive_edt = Some(
                            match EdtSessionManager::for_config(
                                config,
                                EdtSessionHostOptions::for_cli_command(config),
                            ) {
                                Ok(manager) => match EdtDsl::new_shared_session(
                                    edt.clone(),
                                    config.work_path.join("edt-workspace"),
                                    Arc::new(manager),
                                    Duration::from_millis(config.tools.edt_cli.startup_timeout_ms),
                                    Duration::from_millis(config.tools.edt_cli.command_timeout_ms),
                                ) {
                                    Ok(dsl) => dsl.with_execution_policy(context.process_policy(
                                        InterruptionSafetyClass::GracefulThenKill,
                                        None,
                                    )),
                                    Err(error) => {
                                        let app_error = AppError::from(error);
                                        let result = fail_from_source_set_index(
                                            started,
                                            steps,
                                            &ordered_source_sets,
                                            index,
                                            source_set,
                                            BuildMode::EdtExport,
                                            app_error.to_string(),
                                        );
                                        return Err(BuildExecutionFailure::with_payload(
                                            app_error, result,
                                        ));
                                    }
                                },
                                Err(error) => {
                                    let app_error = AppError::from(error);
                                    let result = fail_from_source_set_index(
                                        started,
                                        steps,
                                        &ordered_source_sets,
                                        index,
                                        source_set,
                                        BuildMode::EdtExport,
                                        app_error.to_string(),
                                    );
                                    return Err(BuildExecutionFailure::with_payload(
                                        app_error, result,
                                    ));
                                }
                            },
                        );
                    }
                    execute_edt_export_step(
                        context,
                        config,
                        interactive_edt.as_ref().expect("interactive edt dsl"),
                        source_set,
                        &edt_context,
                        &designer_context,
                        index,
                    )
                } else {
                    let one_shot_edt = EdtDsl::new(
                        edt.clone(),
                        config.work_path.join("edt-workspace"),
                        utilities.runner_for(UtilityType::EdtCli),
                    )
                    .with_execution_policy(
                        context.process_policy(InterruptionSafetyClass::GracefulThenKill, None),
                    );
                    execute_edt_export_step(
                        context,
                        config,
                        &one_shot_edt,
                        source_set,
                        &edt_context,
                        &designer_context,
                        index,
                    )
                };
                let export_warnings = match export_result {
                    Ok(warnings) => warnings,
                    Err(error) => {
                        let result = fail_from_source_set_index(
                            started,
                            steps,
                            &ordered_source_sets,
                            index,
                            source_set,
                            BuildMode::EdtExport,
                            error.to_string(),
                        );
                        return Err(BuildExecutionFailure::with_payload(error, result));
                    }
                };
                if let Err(app_error) =
                    commit_step_state(source_set, &edt_context, &config.work_path, &commit)
                {
                    let result = fail_from_source_set_index(
                        started,
                        steps,
                        &ordered_source_sets,
                        index,
                        source_set,
                        BuildMode::EdtExport,
                        app_error.to_string(),
                    );
                    return Err(BuildExecutionFailure::with_payload(app_error, result));
                }

                push_build_step(
                    &mut steps,
                    &source_set.name,
                    BuildMode::EdtExport,
                    true,
                    merge_step_message("EDT export completed".to_owned(), &export_warnings),
                    export_started.elapsed().as_millis() as u64,
                );
            }
        }

        let designer_stage = match plan_generated_designer_load_step(
            source_set,
            &designer_context,
            args.full_rebuild,
            edt_stage_skipped,
            config.build.partial_load_threshold,
            &config.work_path,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let result = fail_from_source_set_index(
                    started,
                    steps,
                    &ordered_source_sets,
                    index,
                    source_set,
                    BuildMode::Skipped,
                    error.to_string(),
                );
                return Err(BuildExecutionFailure::with_payload(
                    AppError::Runtime(error.to_string()),
                    result,
                ));
            }
        };

        match designer_stage {
            StepPlan::Skip { message, ok } => {
                push_build_step(
                    &mut steps,
                    &source_set.name,
                    BuildMode::Skipped,
                    ok,
                    message,
                    0,
                );
            }
            StepPlan::Execute {
                mode,
                message,
                partial_paths,
                commit,
            } => {
                let load_started = Instant::now();
                let load_result = match config.builder {
                    BuilderBackend::Designer => {
                        let designer = match designer_binary.clone() {
                            Some(path) => path,
                            None => {
                                let location = match utilities.locate(UtilityType::V8) {
                                    Ok(location) => location,
                                    Err(error) => {
                                        let result = fail_from_source_set_index(
                                            started,
                                            steps,
                                            &ordered_source_sets,
                                            index,
                                            source_set,
                                            mode.clone(),
                                            error.to_string(),
                                        );
                                        return Err(BuildExecutionFailure::with_payload(
                                            AppError::from(error),
                                            result,
                                        ));
                                    }
                                };
                                designer_binary = Some(location.path.clone());
                                location.path
                            }
                        };
                        execute_source_set_step(
                            context,
                            config,
                            &designer,
                            utilities.runner_for(UtilityType::V8),
                            source_set,
                            &designer_context,
                            &designer_context,
                            index,
                            partial_paths.as_deref(),
                            &commit,
                            resolve_dynamic_update(config, args),
                        )
                    }
                    BuilderBackend::Ibcmd => {
                        let ibcmd = match ibcmd_binary.clone() {
                            Some(path) => path,
                            None => {
                                let location = match utilities.locate(UtilityType::Ibcmd) {
                                    Ok(location) => location,
                                    Err(error) => {
                                        let result = fail_from_source_set_index(
                                            started,
                                            steps,
                                            &ordered_source_sets,
                                            index,
                                            source_set,
                                            mode.clone(),
                                            error.to_string(),
                                        );
                                        return Err(BuildExecutionFailure::with_payload(
                                            AppError::from(error),
                                            result,
                                        ));
                                    }
                                };
                                ibcmd_binary = Some(location.path.clone());
                                location.path
                            }
                        };
                        execute_source_set_step_ibcmd(
                            context,
                            config,
                            &ibcmd,
                            utilities.runner_for(UtilityType::Ibcmd),
                            source_set,
                            &designer_context,
                            &designer_context,
                            partial_paths.as_deref(),
                            &commit,
                        )
                    }
                };
                match load_result {
                    Ok(warnings) => push_build_step(
                        &mut steps,
                        &source_set.name,
                        mode,
                        true,
                        merge_step_message(message, &warnings),
                        load_started.elapsed().as_millis() as u64,
                    ),
                    Err(error) => {
                        let result = fail_from_source_set_index(
                            started,
                            steps,
                            &ordered_source_sets,
                            index,
                            source_set,
                            mode,
                            error.to_string(),
                        );
                        return Err(BuildExecutionFailure::with_payload(error, result));
                    }
                }
            }
        }
    }

    Ok(BuildResult {
        ok: true,
        steps,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}
