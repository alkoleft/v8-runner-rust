use super::*;

#[cfg(test)]
thread_local! {
    static BEFORE_SOURCE_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn set_before_source_publication_hook(hook: impl FnOnce() + 'static) {
    BEFORE_SOURCE_PUBLICATION.with(|slot| slot.replace(Some(Box::new(hook))));
}

#[cfg(test)]
fn run_before_source_publication_hook() {
    BEFORE_SOURCE_PUBLICATION.with(|slot| {
        if let Some(hook) = slot.take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_before_source_publication_hook() {}

enum PrivateDumpOutcome {
    Applied {
        platform_result: PlatformCommandResult,
        cleanup_message: Option<String>,
        receipt: crate::domain::sync_receipt::SyncReceipt,
    },
    Conflict(crate::domain::sync_receipt::SyncReceipt),
}

fn effective_dump_mode(mode: EffectiveDumpMode) -> DumpMode {
    match mode {
        EffectiveDumpMode::Full => DumpMode::Full,
        EffectiveDumpMode::Incremental => DumpMode::Incremental,
        EffectiveDumpMode::Partial => DumpMode::Partial,
    }
}

fn observed_generation(context: &SourceSetContext) -> Result<u64, AppError> {
    match HashStorage::new(context.storage_path())
        .observe_state()
        .map_err(|error| AppError::Runtime(error.to_string()))?
    {
        ObservedHashStorage::MissingPath | ObservedHashStorage::ExistingUninitialized(_) => Ok(0),
        ObservedHashStorage::Initialized(snapshot) => Ok(snapshot.generation),
        ObservedHashStorage::Recoverable(observation) => Ok(observation.generation()),
    }
}

fn current_baseline_manifest(
    context: &SourceSetContext,
    generation: StateGeneration,
) -> Result<BTreeMap<String, [u8; 32]>, AppError> {
    match inspect_baseline(&context.baseline(BaselineRole::ConfiguredSource, generation))
        .map_err(|error| AppError::Runtime(error.to_string()))?
    {
        BaselineInspection::Valid(baseline) => Ok(baseline
            .files()
            .iter()
            .map(|file| (file.path().to_owned(), file.sha256()))
            .collect()),
        BaselineInspection::Missing | BaselineInspection::Corrupt(_) => Ok(BTreeMap::new()),
    }
}

fn baseline_is_valid(
    context: &SourceSetContext,
    role: BaselineRole,
    generation: StateGeneration,
) -> Result<bool, AppError> {
    Ok(matches!(
        inspect_baseline(&context.baseline(role, generation))
            .map_err(|error| AppError::Runtime(error.to_string()))?,
        BaselineInspection::Valid(_)
    ))
}

fn resolved_for_shadows(
    resolved: &ResolvedDumpTarget,
    configured_shadow: &Path,
    platform_shadow: &Path,
) -> Result<ResolvedDumpTarget, AppError> {
    if resolved.platform_target_path != resolved.platform_designer_context.path() {
        return Err(AppError::Runtime(
            "resolved platform target does not match its Designer context".to_owned(),
        ));
    }
    let mut shadow = resolved.clone();
    shadow.target_path = configured_shadow.to_path_buf();
    shadow.canonical_target_path = nearest_existing_canonical_path(configured_shadow)
        .map_err(|error| AppError::Runtime(format!("failed to resolve dump shadow: {error}")))?;
    shadow.target_identity = stable_path_identity(&shadow.canonical_target_path);
    shadow.platform_target_path = platform_shadow.to_path_buf();
    shadow.canonical_platform_target_path = nearest_existing_canonical_path(platform_shadow)
        .map_err(|error| {
            AppError::Runtime(format!("failed to resolve platform shadow: {error}"))
        })?;
    shadow.platform_target_identity = stable_path_identity(&shadow.canonical_platform_target_path);
    Ok(shadow)
}

fn prepare_dump_observation(
    observed: &PreparedStateUpdate,
    post_publication: &ManagedInventory,
    merge: &ManifestMergePlan,
) -> Result<PreparedStateUpdate, AppError> {
    let current = post_publication
        .current
        .iter()
        .map(|file| (file.rel_path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let retain_local = merge
        .entries()
        .iter()
        .any(|entry| entry.action() == MergeAction::RetainLocal);
    let mut snapshot = Vec::new();
    for entry in merge.entries() {
        let version = match entry.action() {
            MergeAction::Apply | MergeAction::Converged | MergeAction::NoOp => entry.dump(),
            MergeAction::RetainLocal => entry.baseline(),
            MergeAction::Conflict => {
                return Err(AppError::Runtime(
                    "conflicted merge cannot prepare dump observation".to_owned(),
                ))
            }
        };
        let FileVersion::Present(hash) = version else {
            continue;
        };
        let hash = hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mtime_ns = current
            .get(entry.path())
            .filter(|file| file.hash == hash)
            .map_or(0, |file| file.mtime_ns);
        snapshot.push(PreparedFileState {
            rel_path: entry.path().to_owned(),
            mtime_ns,
            hash,
        });
    }
    Ok(PreparedStateUpdate {
        snapshot,
        scan_started_at: if retain_local {
            0
        } else {
            post_publication.prepared.scan_started_at
        },
        observed_storage: observed.observed_storage.clone(),
    })
}

fn rollback_publication_failure(
    failure: AppError,
    transaction_root: &Path,
    context: &SourceSetContext,
    identity: &TargetIdentity,
    observed_generation: u64,
) -> AppError {
    match recover_publication(
        transaction_root,
        context.path(),
        identity,
        ObservedStateGeneration::new(observed_generation),
    ) {
        Ok(()) => failure,
        Err(recovery_error) => AppError::Runtime(format!(
            "dump operation failed ({failure}); source rollback also failed ({recovery_error})"
        )),
    }
}

fn observed_publication_state(
    context: &SourceSetContext,
    generation: u64,
) -> Result<ObservedStateGeneration, AppError> {
    match HashStorage::new(context.storage_path())
        .current_dump_transaction_id()
        .map_err(|error| AppError::Runtime(error.to_string()))?
    {
        Some(transaction_id) => Ok(ObservedStateGeneration::with_dump_transaction(
            generation,
            transaction_id,
        )),
        None => Ok(ObservedStateGeneration::new(generation)),
    }
}

pub(super) fn run_dump_with_context(
    context: &ExecutionContext,
    config: &AppConfig,
    args: &DumpArgs,
) -> UseCaseResult<DumpResult> {
    let started = Instant::now();
    let mode = match args.mode {
        DumpModeRequest::Full => DumpMode::Full,
        DumpModeRequest::Incremental => DumpMode::Incremental,
        DumpModeRequest::Partial => DumpMode::Partial,
    };
    debug!(
        mode = ?mode,
        source_set = args.source_set.as_deref().unwrap_or("<auto>"),
        extension = args.extension.as_deref().unwrap_or("<none>"),
        "starting dump"
    );

    if let Some(error) = validate_supported_matrix(config) {
        return Err(DumpExecutionFailure::with_payload(
            error,
            empty_result(
                mode,
                started,
                None,
                None,
                None,
                None,
                Some(SUPPORTED_DUMP_ERROR.to_owned()),
            ),
        ));
    }

    let partial_objects = match validate_dump_objects(&mode, &args.objects) {
        Ok(objects) => objects,
        Err(error) => {
            let message = error.to_string();
            return Err(DumpExecutionFailure::with_payload(
                error,
                empty_result(
                    mode,
                    started,
                    args.source_set.clone(),
                    args.extension.clone(),
                    None,
                    None,
                    Some(message),
                ),
            ));
        }
    };
    let selectors = partial_objects.as_ref().map(|objects| {
        objects
            .iter()
            .map(|selector| DumpSelectorResult {
                requested: selector.requested().to_owned(),
                normalized: selector.normalized(),
            })
            .collect()
    });

    let resolved = match resolve_target(config, args) {
        Ok(resolved) => resolved,
        Err(error) => {
            let message = error.to_string();
            return Err(DumpExecutionFailure::with_payload(
                error,
                empty_result(
                    mode,
                    started,
                    args.source_set.clone(),
                    args.extension.clone(),
                    selectors.clone(),
                    None,
                    Some(message),
                ),
            ));
        }
    };

    let mut utilities = PlatformUtilities::from_config(config);
    let utility = match config.builder {
        BuilderBackend::Designer => UtilityType::V8,
        BuilderBackend::Ibcmd => UtilityType::Ibcmd,
    };
    let location = match utilities.locate(utility) {
        Ok(location) => location,
        Err(error) => {
            let message = error.to_string();
            let app_error = AppError::from(error);
            return Err(DumpExecutionFailure::with_payload(
                app_error,
                empty_result(
                    mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    selectors.clone(),
                    Some(resolved.target_path.clone()),
                    Some(message),
                ),
            ));
        }
    };
    let edt_binary = if config.format == SourceFormat::Edt {
        Some(match utilities.locate(UtilityType::EdtCli) {
            Ok(location) => location.path,
            Err(error) => {
                let message = error.to_string();
                let app_error = AppError::from(error);
                return Err(DumpExecutionFailure::with_payload(
                    app_error,
                    empty_result(
                        mode,
                        started,
                        Some(resolved.source_set_name.clone()),
                        resolved.extension.clone(),
                        selectors.clone(),
                        Some(resolved.target_path.clone()),
                        Some(message),
                    ),
                ));
            }
        })
    } else {
        None
    };

    let lock_guard = match acquire_advisory_lock(&resolved.lock_path) {
        Ok(lock_guard) => lock_guard,
        Err(error) => {
            let message = format!(
                "failed to acquire dump lock '{}': {error}",
                resolved.lock_path.display()
            );
            let app_error = AppError::Runtime(message.clone());
            return Err(DumpExecutionFailure::with_payload(
                app_error,
                empty_result(
                    mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    selectors.clone(),
                    Some(resolved.target_path.clone()),
                    Some(message),
                ),
            ));
        }
    };

    if let Err(error) = cleanup_orphan_dirs(&resolved) {
        let message = format!("failed to cleanup stale dump temp dirs: {error}");
        let app_error = AppError::Runtime(message.clone());
        return Err(DumpExecutionFailure::with_payload(
            app_error,
            empty_result(
                mode,
                started,
                Some(resolved.source_set_name.clone()),
                resolved.extension.clone(),
                selectors.clone(),
                Some(resolved.target_path.clone()),
                Some(message),
            ),
        ));
    }
    if resolved.platform_target_path != resolved.target_path {
        if let Err(error) = cleanup_platform_orphan_dirs(&resolved) {
            let message = format!("failed to cleanup stale dump platform temp dirs: {error}");
            let app_error = AppError::Runtime(message.clone());
            return Err(DumpExecutionFailure::with_payload(
                app_error,
                empty_result(
                    mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    selectors.clone(),
                    Some(resolved.target_path.clone()),
                    Some(message),
                ),
            ));
        }
    }

    if let Err(error) = validate_publish_target(&resolved) {
        let message = error.to_string();
        return Err(DumpExecutionFailure::with_payload(
            error,
            empty_result(
                mode,
                started,
                Some(resolved.source_set_name.clone()),
                resolved.extension.clone(),
                selectors.clone(),
                Some(resolved.target_path.clone()),
                Some(message),
            ),
        ));
    }
    if resolved.platform_target_path != resolved.target_path {
        if let Err(error) = validate_platform_target(&resolved) {
            let message = error.to_string();
            return Err(DumpExecutionFailure::with_payload(
                error,
                empty_result(
                    mode,
                    started,
                    Some(resolved.source_set_name.clone()),
                    resolved.extension.clone(),
                    selectors.clone(),
                    Some(resolved.target_path.clone()),
                    Some(message),
                ),
            ));
        }
    }

    let mut failure_receipt = crate::domain::sync_receipt::SyncReceipt::empty_failed();
    let result = (|| -> Result<PrivateDumpOutcome, AppError> {
        let state_lock = lock_designer_state(&resolved.configured_source_context)
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        recover_designer_state_with_lock(&resolved.configured_source_context, &state_lock)
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        ensure_dir(resolved.configured_source_context.path()).map_err(|error| {
            AppError::Runtime(format!("failed to create configured source root: {error}"))
        })?;
        let observed_generation = observed_generation(&resolved.configured_source_context)?;
        let generation = StateGeneration::new(observed_generation);
        let publication_root = resolved
            .configured_source_context
            .transactions_dir()
            .join("source-publication");
        let publication_identity = TargetIdentity::new(resolved.target_identity.clone());
        recover_publication(
            &publication_root,
            resolved.configured_source_context.path(),
            &publication_identity,
            observed_publication_state(&resolved.configured_source_context, observed_generation)?,
        )
        .map_err(|error| AppError::Runtime(error.to_string()))?;
        let inventory = crate::change_detection::analyzer::managed_inventory(
            &resolved.configured_source_context,
        )
        .map_err(|error| AppError::Runtime(error.to_string()))?;
        if inventory.prepared.observed_storage.generation() != observed_generation {
            return Err(AppError::Runtime(
                "runtime-state generation changed during dump recovery".to_owned(),
            ));
        }

        let baseline = current_baseline_manifest(&resolved.configured_source_context, generation)?;
        let source = managed_manifest(
            resolved.configured_source_context.path(),
            resolved.configured_source_context.excluded_roots(),
        )
        .map_err(|error| AppError::Runtime(error.to_string()))?;
        let shadow_request = if config.format == SourceFormat::Edt
            && !matches!(args.mode, DumpModeRequest::Full)
            && !baseline_is_valid(
                &resolved.configured_source_context,
                BaselineRole::ConfiguredSource,
                generation,
            )? {
            DumpModeRequest::Full
        } else {
            args.mode
        };
        let platform_shadow = DumpShadow::prepare(
            &resolved.configured_source_context,
            if config.format == SourceFormat::Edt {
                BaselineRole::EdtPlatformDesigner
            } else {
                BaselineRole::ConfiguredSource
            },
            generation,
            shadow_request,
        )
        .map_err(|error| AppError::Runtime(error.to_string()))?;
        let configured_shadow = if config.format == SourceFormat::Edt {
            Some(
                DumpShadow::prepare(
                    &resolved.configured_source_context,
                    BaselineRole::ConfiguredSource,
                    generation,
                    DumpModeRequest::Full,
                )
                .map_err(|error| AppError::Runtime(error.to_string()))?,
            )
        } else {
            None
        };
        let configured_shadow_root = configured_shadow
            .as_ref()
            .map_or(platform_shadow.path(), DumpShadow::path);
        let shadow_resolved =
            resolved_for_shadows(&resolved, configured_shadow_root, platform_shadow.path())?;
        let effective_mode = effective_dump_mode(platform_shadow.mode());
        let partial_objects = if platform_shadow.mode() == EffectiveDumpMode::Full {
            None
        } else {
            partial_objects.as_deref()
        };
        let baseline_paths = baseline.keys().cloned().collect::<BTreeSet<_>>();
        let write_scope = if platform_shadow.mode() == EffectiveDumpMode::Full {
            EffectiveWriteScope::Full {
                baseline: &baseline_paths,
            }
        } else {
            EffectiveWriteScope::Incremental
        };
        let shadow_observation =
            ShadowObservation::normalize_and_capture(configured_shadow_root, &[])
                .map_err(|error| AppError::Runtime(error.to_string()))?;
        let edt_binary = edt_binary.as_deref();
        let command_result = match (
            config.format,
            &effective_mode,
            &config.builder,
            partial_objects,
            edt_binary,
        ) {
            (SourceFormat::Designer, DumpMode::Incremental, BuilderBackend::Designer, _, _) => {
                run_incremental_dump_designer(
                    context,
                    config,
                    &shadow_resolved,
                    location.path.as_path(),
                    utilities.runner_for(UtilityType::V8),
                )
            }
            (SourceFormat::Designer, DumpMode::Incremental, BuilderBackend::Ibcmd, _, _) => {
                run_incremental_dump_ibcmd(
                    context,
                    config,
                    &shadow_resolved,
                    location.path.as_path(),
                    utilities.runner_for(UtilityType::Ibcmd),
                )
            }
            (SourceFormat::Designer, DumpMode::Full, BuilderBackend::Designer, _, _) => {
                run_full_dump_designer(
                    context,
                    config,
                    &shadow_resolved,
                    location.path.as_path(),
                    utilities.runner_for(UtilityType::V8),
                )
            }
            (SourceFormat::Designer, DumpMode::Full, BuilderBackend::Ibcmd, _, _) => {
                run_full_dump_ibcmd(
                    context,
                    config,
                    &shadow_resolved,
                    location.path.as_path(),
                    utilities.runner_for(UtilityType::Ibcmd),
                )
            }
            (
                SourceFormat::Designer,
                DumpMode::Partial,
                BuilderBackend::Designer,
                Some(objects),
                _,
            ) => run_partial_dump_designer(
                context,
                config,
                &shadow_resolved,
                location.path.as_path(),
                utilities.runner_for(UtilityType::V8),
                objects,
            ),
            (
                SourceFormat::Designer,
                DumpMode::Partial,
                BuilderBackend::Ibcmd,
                Some(objects),
                _,
            ) => run_partial_dump_ibcmd(
                context,
                config,
                &shadow_resolved,
                location.path.as_path(),
                utilities.runner_for(UtilityType::Ibcmd),
                objects,
            ),
            (
                SourceFormat::Edt,
                DumpMode::Incremental,
                BuilderBackend::Designer,
                _,
                Some(edt_binary),
            ) => run_incremental_dump_edt_designer(
                context,
                config,
                &shadow_resolved,
                location.path.as_path(),
                edt_binary,
                utilities.runner_for(UtilityType::V8),
                utilities.runner_for(UtilityType::EdtCli),
            ),
            (
                SourceFormat::Edt,
                DumpMode::Incremental,
                BuilderBackend::Ibcmd,
                _,
                Some(edt_binary),
            ) => run_incremental_dump_edt_ibcmd(
                context,
                config,
                &shadow_resolved,
                location.path.as_path(),
                edt_binary,
                utilities.runner_for(UtilityType::Ibcmd),
                utilities.runner_for(UtilityType::EdtCli),
            ),
            (SourceFormat::Edt, DumpMode::Full, BuilderBackend::Designer, _, Some(edt_binary)) => {
                run_full_dump_edt_designer(
                    context,
                    config,
                    &shadow_resolved,
                    location.path.as_path(),
                    edt_binary,
                    utilities.runner_for(UtilityType::V8),
                    utilities.runner_for(UtilityType::EdtCli),
                )
            }
            (SourceFormat::Edt, DumpMode::Full, BuilderBackend::Ibcmd, _, Some(edt_binary)) => {
                run_full_dump_edt_ibcmd(
                    context,
                    config,
                    &shadow_resolved,
                    location.path.as_path(),
                    edt_binary,
                    utilities.runner_for(UtilityType::Ibcmd),
                    utilities.runner_for(UtilityType::EdtCli),
                )
            }
            (
                SourceFormat::Edt,
                DumpMode::Partial,
                BuilderBackend::Designer,
                Some(objects),
                Some(edt_binary),
            ) => run_partial_dump_edt_designer(
                context,
                config,
                &shadow_resolved,
                location.path.as_path(),
                edt_binary,
                utilities.runner_for(UtilityType::V8),
                utilities.runner_for(UtilityType::EdtCli),
                objects,
            ),
            (
                SourceFormat::Edt,
                DumpMode::Partial,
                BuilderBackend::Ibcmd,
                Some(objects),
                Some(edt_binary),
            ) => run_partial_dump_edt_ibcmd(
                context,
                config,
                &shadow_resolved,
                location.path.as_path(),
                edt_binary,
                utilities.runner_for(UtilityType::Ibcmd),
                utilities.runner_for(UtilityType::EdtCli),
                objects,
            ),
            (_, DumpMode::Partial, _, None, _) => Err(AppError::Runtime(
                "partial dump objects were not validated before execution".to_owned(),
            )),
            (SourceFormat::Edt, _, _, _, None) => Err(AppError::Runtime(
                "EDT binary must be resolved before executing format=EDT dump".to_owned(),
            )),
        };
        let (platform_result, cleanup_message) = match command_result {
            Ok(result) => result,
            Err(error) => return Err(error),
        };
        let dump = managed_manifest(configured_shadow_root, &[])
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        let merge = plan_manifest_merge(&baseline, &source, &dump);
        failure_receipt = merge
            .failed_receipt()
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        if merge.has_conflicts() {
            return Ok(PrivateDumpOutcome::Conflict(
                merge
                    .conflict_receipt()
                    .map_err(|error| AppError::Runtime(error.to_string()))?,
            ));
        }
        let writes = shadow_observation
            .observe_writes(configured_shadow_root, &[], write_scope)
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        let next_generation = observed_generation
            .checked_add(1)
            .ok_or_else(|| AppError::Runtime("runtime-state generation overflow".to_owned()))?;
        let dump_transaction_id = DumpTransactionId::new();
        let prepared_publication = PublicationRequest::builder(
            resolved.configured_source_context.path(),
            configured_shadow_root,
            &merge,
        )
        .transaction_root(&publication_root)
        .generation(next_generation)
        .target_identity(publication_identity.clone())
        .dump_transaction_id(dump_transaction_id.clone())
        .prepare()
        .map_err(|error| AppError::Runtime(error.to_string()))?;
        run_before_source_publication_hook();
        if let Some(error) = interruption_before_publish(context, "dump source publication") {
            return Err(rollback_publication_failure(
                error,
                &publication_root,
                &resolved.configured_source_context,
                &publication_identity,
                observed_generation,
            ));
        }
        let source_applied = prepared_publication
            .apply()
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        let post_publication = match crate::change_detection::analyzer::managed_inventory(
            &resolved.configured_source_context,
        ) {
            Ok(inventory) => inventory,
            Err(error) => {
                return Err(rollback_publication_failure(
                    AppError::Runtime(error.to_string()),
                    &publication_root,
                    &resolved.configured_source_context,
                    &publication_identity,
                    observed_generation,
                ))
            }
        };
        let prepared_state =
            match prepare_dump_observation(&inventory.prepared, &post_publication, &merge) {
                Ok(prepared) => prepared,
                Err(error) => {
                    return Err(rollback_publication_failure(
                        error,
                        &publication_root,
                        &resolved.configured_source_context,
                        &publication_identity,
                        observed_generation,
                    ))
                }
            };
        let produced_cdfi = platform_shadow.path().join("ConfigDumpInfo.xml");
        let mut commit_request =
            DumpStateCommitRequest::new(&prepared_state, configured_shadow_root, &produced_cdfi)
                .with_transaction_id(dump_transaction_id.clone());
        if config.format == SourceFormat::Edt {
            commit_request = commit_request.with_edt_platform_designer(platform_shadow.path());
        }
        let visible_generation = match commit_dump_state_with_lock(
            &resolved.configured_source_context,
            &state_lock,
            commit_request,
        ) {
            Ok(generation) => generation,
            Err(commit_error) => {
                return Err(rollback_publication_failure(
                    AppError::Runtime(commit_error.to_string()),
                    &publication_root,
                    &resolved.configured_source_context,
                    &publication_identity,
                    observed_generation,
                ));
            }
        };
        if let Err(publication_error) = source_applied
            .mark_state_visible(ObservedStateGeneration::with_dump_transaction(
                visible_generation.value(),
                dump_transaction_id.clone(),
            ))
            .and_then(|publication| publication.commit())
        {
            recover_publication(
                &publication_root,
                resolved.configured_source_context.path(),
                &publication_identity,
                ObservedStateGeneration::with_dump_transaction(
                    visible_generation.value(),
                    dump_transaction_id,
                ),
            )
            .map_err(|recovery_error| {
                AppError::Runtime(format!(
                    "source publication finalization failed ({publication_error}); recovery also failed ({recovery_error})"
                ))
            })?;
        }
        Ok(PrivateDumpOutcome::Applied {
            platform_result,
            cleanup_message,
            receipt: merge
                .applied_receipt(&writes)
                .map_err(|error| AppError::Runtime(error.to_string()))?,
        })
    })();
    drop(lock_guard);

    match result {
        Ok(PrivateDumpOutcome::Applied {
            platform_result,
            cleanup_message,
            receipt,
        }) => Ok(DumpResult {
            ok: true,
            source_set: Some(resolved.source_set_name),
            extension: resolved.extension,
            selectors,
            mode,
            target_path: resolved.target_path,
            platform_log_path: platform_result.platform_log_path,
            duration_ms: started.elapsed().as_millis() as u64,
            message: cleanup_message.or_else(|| Some("dump completed successfully".to_owned())),
            receipt,
        }),
        Ok(PrivateDumpOutcome::Conflict(receipt)) => {
            let message = "dump publication conflicts with local source changes".to_owned();
            Err(DumpExecutionFailure::with_payload(
                AppError::Runtime(message.clone()),
                DumpResult {
                    ok: false,
                    source_set: Some(resolved.source_set_name),
                    extension: resolved.extension,
                    selectors,
                    mode,
                    target_path: resolved.target_path,
                    platform_log_path: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    message: Some(message),
                    receipt,
                },
            ))
        }
        Err(error) => {
            let message = error.to_string();
            Err(DumpExecutionFailure::with_payload(
                error,
                DumpResult {
                    ok: false,
                    source_set: Some(resolved.source_set_name),
                    extension: resolved.extension,
                    selectors,
                    mode,
                    target_path: resolved.target_path,
                    platform_log_path: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    message: Some(message),
                    receipt: failure_receipt,
                },
            ))
        }
    }
}
