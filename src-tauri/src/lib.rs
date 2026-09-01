pub mod commands;
pub mod content;
pub mod db;
pub mod discovery; // NEXT-048 发现五断面数据面（Bridge 写 + 只读 feed）
pub mod export;
pub mod global_search; // NB-14 全局搜索（轨道 A 只读扩展文件）
pub mod knowledge;
pub mod local_terminal;
pub mod local_workspace;
pub mod notes;
pub mod openrouter_rankings;
pub mod project_tree; // NB-19 项目内文档组织树（轨道 A 用户指令例外，AG-11 落地）
pub mod scheduler;
pub mod storage_gc; // NB-12 存储治理（轨道 A §3.9 备案例外，独立新文件）
pub mod storage_layout;
pub mod vector;

// ---- Track B · 智能体演进（AG-01 追加）：Agent Runtime 模型层，见 docs/architecture.md ----
pub mod model;
// ---- Track B · 智能体演进（AG-02 追加）：AI 工作室项目容器，见 docs/architecture.md ----
pub mod projects;
// ---- Track B · 智能体演进（AG-04 追加）：Agent Runtime 外壳（Phase 1 Spike），见 docs/architecture.md ----
pub mod agent;
// ---- Track B · 智能体演进（AG-06 追加）：ToolGateway 工具层（Spike 假工具），见 docs/architecture.md ----
pub mod tools;
// ---- Track B · AG-30 追加：独立 CompletionService（低延迟补全，不建 Thread/Run），见 docs/architecture.md ----
pub mod completion;
// ---- Track B · AG-24 追加：Phase 3 DocumentService（Repository/Service/用户命令），见 docs/architecture.md ----
pub mod documents;
// ---- Track B · AG-27 追加：Phase 4 Skill 系统（Loader/Resolver/启用态/权限交集），见 docs/architecture.md ----
pub mod skills;
// ---- H5 / NEXT-022：SophoNote MCP Bridge + SidecarLease（DEC-012 模型只配一次）----
pub mod sophonote_mcp;

use tauri::Manager;

pub struct AppState {
    pub scheduler: scheduler::SchedulerHandle,
    pub hermes: tokio::sync::Mutex<Option<agent::hermes::bundled_runtime::BundledHermesRuntime>>,
    /// Hermes 健康监督器：后台保活、自动重连、向前端推送状态。
    pub hermes_health:
        std::sync::Mutex<Option<agent::hermes::health_supervisor::HermesHealthSupervisor>>,
}

pub async fn restart_bundled_hermes(app: &tauri::AppHandle) -> Result<(), String> {
    if agent::hermes::bundled_runtime::should_use_external_debug_gateway() {
        return Ok(());
    }
    let state = app.state::<AppState>();
    let mut runtime = state.hermes.lock().await;
    if let Some(mut previous) = runtime.take() {
        previous.shutdown();
    }
    let next = agent::hermes::bundled_runtime::start(app).await?;
    *runtime = Some(next);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 崩溃取证：panic 发生时先打印可在 dev.log 中检索的标记，再走默认 hook（打印位置/回溯）。
    // 若 dev.log 中出现该标记 → 进程内 panic；若日志戛然而止且无任何退出标记 → 进程被异常终止
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("[sophonote] !! PANIC: {}", info);
        default_hook(info);
    }));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_sql::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Register sqlite-vec extension before opening any connection
            vector::register_vec_extension();

            // 品牌迁移（MindBox → SophoNote）：Bundle ID 切换后首次启动，
            // 将旧 com.fei.mindbox 数据根整体复制到新根（含 mindbox.db → sophonote.db），
            // 旧目录保留作为回滚备份；必须在 ensure/init_db 之前执行。
            match storage_layout::StorageLayout::migrate_legacy_root(app.handle()) {
                Ok(true) => println!(
                    "[storage] 已复制 MindBox 旧数据根 com.fei.mindbox → com.fei.sophonote（旧目录保留）"
                ),
                Ok(false) => {}
                Err(error) => eprintln!("[storage] 旧数据根迁移失败: {error}"),
            }

            // 所有持久/运行目录从同一数据根解析。这里只幂等补齐新增分区，
            // 数据根切换由上方 migrate_legacy_root 完成。
            storage_layout::StorageLayout::resolve(app.handle())
                .and_then(|layout| layout.ensure())
                .map_err(std::io::Error::other)?;

            // Initialize database
            if let Err(e) = db::init_db(app.handle()) {
                eprintln!("Database init error: {}", e);
            }

            // N0：笔记存储迁移（DB content → .md 文件 + data URL 图片落盘），同步执行，
            // 必须在窗口加载（前端读 articles）之前完成；幂等，二次启动仅校验
            notes::migrate_articles_to_files(app.handle());

            // AG-24（docs/architecture.md）：启动恢复——扫描上次未完成的文档写操作
            // （status=prepared：tmp 已写、rename 中断），清 tmp 残留并回滚状态。
            // 正文文件未被动过（rename 原子），回滚即安全；committed 是真相不碰
            if let Ok(conn) = rusqlite::Connection::open(db::get_db_path(app.handle())) {
                let n = documents::service::recover_pending_operations(
                    &conn,
                    &notes::notes_dir(app.handle()),
                );
                if n > 0 {
                    println!("[documents] startup recovery: rolled back {n} pending operation(s)");
                }
            }

            // Start background scheduler
            let scheduler = scheduler::start_scheduler(app.handle().clone());
            app.manage(AppState {
                scheduler,
                hermes: tokio::sync::Mutex::new(None),
                hermes_health: std::sync::Mutex::new(None),
            });
            app.manage(local_terminal::TerminalManager::new());

            // Release 必须在窗口可用前完成包内 Runtime 校验与启动；不存在机器 Hermes 回退。
            // Debug 显式附着外部 Gateway 时跳过，否则同样从 resources 启动以便 D3 复验。
            if !agent::hermes::bundled_runtime::should_use_external_debug_gateway() {
                if let Err(error) =
                    tauri::async_runtime::block_on(restart_bundled_hermes(app.handle()))
                {
                    #[cfg(not(debug_assertions))]
                    return Err(std::io::Error::other(format!(
                        "包内 Hermes Runtime 未就绪，Release 拒绝启动: {error}"
                    ))
                    .into());
                    #[cfg(debug_assertions)]
                    eprintln!("[hermes] bundled debug runtime unavailable: {error}");
                }
                // 启动 Hermes 健康监督器：后台轮询 /api/health，连续失败自动重连，
                // 并通过 Tauri Event `sophonote:hermes-status-changed` 向前端推送状态。
                let supervisor = agent::hermes::health_supervisor::HermesHealthSupervisor::start(
                    app.handle().clone(),
                );
                if let Ok(mut guard) = app.state::<AppState>().hermes_health.lock() {
                    *guard = Some(supervisor);
                }
            } else if let Err(error) =
                agent::hermes::bundled_runtime::configure_external_debug_surface(app.handle())
            {
                return Err(std::io::Error::other(format!(
                    "外部 Hermes Surface 配置失败: {error}"
                ))
                .into());
            }

            // 发现页把已保存 deep 作为用户可见内容的硬门禁。用户已明确授权由
            // Hermes Cron 自动补全，因此启动后只通过 Cron 原生 API 做一次幂等对账：
            // 缺失时创建任务，已有任务则不覆盖用户的启停、时间或模型选择。
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if let Err(error) =
                        agent::commands::reconcile_hermes_cron_jobs(&app_handle).await
                    {
                        eprintln!(
                            "[hermes] discovery deep-backfill cron reconciliation failed: {error}"
                        );
                    }
                });
            }

            // 启动后：先离线转论文正文（A4，不联网），再按来源配额补抓存量正文（A2），错峰 15s
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                    // A4：arXiv/HF Papers 完整摘要离线转正文，论文覆盖率立即达标
                    match content::convert_papers_offline(&app_handle) {
                        Ok(report) => println!("[content] offline paper conversion: {}", report),
                        Err(e) => eprintln!("[content] offline paper conversion error: {}", e),
                    }
                    // A2：按来源配额补抓（论文已离线转好，此处主要推进 GitHub/HN/HF 模型）
                    match content::backfill_contents(&app_handle, 10).await {
                        Ok(report) => println!("[content] startup backfill: {}", report),
                        Err(e) => eprintln!("[content] startup backfill error: {}", e),
                    }
                });
            }

            // Setup tray icon (desktop only)
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
                use tauri::tray::TrayIconBuilder;

                let show_i = MenuItem::with_id(app, "show", "显示 SophoNote", true, None::<&str>)?;
                let hide_i = MenuItem::with_id(app, "hide", "隐藏", true, None::<&str>)?;
                let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
                let menu = Menu::with_items(
                    app,
                    &[
                        &show_i,
                        &hide_i,
                        &PredefinedMenuItem::separator(app)?,
                        &quit_i,
                    ],
                )?;

                let tray = TrayIconBuilder::new()
                    .menu(&menu)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "hide" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.hide();
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        use tauri::tray::MouseButton;
                        use tauri::tray::TrayIconEvent;
                        if let TrayIconEvent::Click { button, .. } = event {
                            if button == MouseButton::Left {
                                let app = tray.app_handle();
                                if let Some(window) = app.get_webview_window("main") {
                                    if window.is_visible().unwrap_or(false) {
                                        let _ = window.hide();
                                    } else {
                                        let _ = window.show();
                                        let _ = window.set_focus();
                                    }
                                }
                            }
                        }
                    })
                    .build(app);

                if let Err(e) = tray {
                    eprintln!("Tray icon setup error: {}", e);
                }
            }

            // 启动完成标记：dev.log 中每次出现该行 = 一次成功启动（含 pid），
            // 同一段日志里出现多次 = dev 监听器因 Rust 改动自动重启了进程（感知上的「闪退」主因之一）
            println!(
                "[sophonote] startup complete: v{} pid={} at {}",
                env!("CARGO_PKG_VERSION"),
                std::process::id(),
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );

            Ok(())
        })
        .on_window_event(|window, event| {
            // AG-23：单分支 match → if let（Clippy single_match）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                {
                    window.hide().ok();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::db_init,
            commands::db_get_items,
            commands::db_insert_item,
            commands::db_update_item_status,
            commands::db_delete_item,
            commands::db_get_sources,
            commands::db_toggle_source,
            commands::db_update_source_interval,
            commands::db_update_source_discovery_config,
            commands::db_update_item_ai,
            commands::db_get_item_enrich,
            commands::db_insert_log,
            commands::db_get_logs,
            commands::db_get_tasks,
            commands::db_insert_task,
            commands::db_delete_task,
            commands::db_insert_pomodoro_session,
            commands::db_list_pomodoro_sessions,
            commands::db_get_stats,
            commands::update_setting,
            commands::get_setting,
            commands::get_app_version,
            commands::fetch_sources_now,
            commands::get_item_content,
            commands::content_coverage_stats,
            commands::rebuild_stories,
            commands::get_stories,
            commands::get_content_cached,
            commands::db_update_source_tier,
            commands::db_update_source_admission,
            commands::convert_papers_offline,
            commands::backfill_item_contents,
            commands::get_data_dir,
            commands::get_storage_layout,
            commands::app_update_check,
            commands::app_update_install,
            commands::hermes_sidecar_status,
            commands::hermes_sidecar_pull,
            commands::keychain_save_api_key,
            commands::keychain_get_api_key,
            commands::keychain_delete_api_key,
            commands::ai_generate_embedding,
            commands::db_insert_article,
            commands::db_update_article,
            commands::db_rename_article,
            commands::db_delete_article,
            commands::db_delete_articles,
            commands::db_get_articles,
            commands::db_get_deep_dive_by_item,
            notes::save_note_asset,
            notes::read_note_asset,
            commands::db_get_pick_candidates,
            commands::db_save_daily_picks,
            commands::db_get_daily_picks,
            commands::db_discovery_feed,
            commands::db_discovery_topics_summary,
            commands::db_model_leaderboard,
            openrouter_rankings::db_openrouter_rankings,
            openrouter_rankings::openrouter_rankings_refresh,
            commands::db_discovery_reports,
            local_workspace::local_workspace_scan,
            local_workspace::local_workspace_list_directory,
            local_workspace::local_workspace_read,
            local_workspace::local_workspace_preview_file,
            local_workspace::local_workspace_git_status,
            local_workspace::local_workspace_git_diff,
            local_workspace::local_workspace_write,
            local_workspace::local_workspace_git_stage,
            local_workspace::local_workspace_git_unstage,
            local_workspace::local_workspace_git_discard,
            local_workspace::local_workspace_run_command,
            local_terminal::local_terminal_create,
            local_terminal::local_terminal_write,
            local_terminal::local_terminal_resize,
            local_terminal::local_terminal_close,
            vector::vec_upsert_embedding,
            vector::vec_search,
            vector::vec_index_stats,
            vector::vec_indexed_ids,
            vector::vec_delete_embedding,
            vector::vec_upsert_chunks,
            vector::vec_search_chunks,
            vector::vec_chunk_indexed_ids,
            vector::vec_upsert_note_chunks,
            vector::vec_search_note_chunks,
            vector::vec_note_chunk_indexed_ids,
            // ===== 轨道 A 命令（NB-xx · 垂类笔记，只追加不重排，见 docs/architecture.md） =====
            export::export_notebook,
            export::export_article, // NB-13 单篇导出（三空间右键菜单）
            storage_gc::notebook_storage_stats, // NB-12 只读统计
            storage_gc::gc_orphan_assets, // NB-12 孤儿清理（§3.9 备案例外）
            global_search::global_search, // NB-14 三域融合检索（只读）
            project_tree::project_set_doc_parent, // NB-19 组织树置父原语（用户指令例外，AG-11 落地）
            // ===== 轨道 B 命令（AG-xx · 智能体演进，只追加不重排，见 docs/architecture.md） =====
            model::commands::ai_chat_completion,
            model::commands::ai_test_chat_connection,
            model::commands::ai_provider_models,
            projects::project_list,
            projects::project_create,
            projects::project_rename,
            projects::project_set_pinned,
            projects::project_set_description,
            projects::project_delete,
            projects::project_list_memberships,
            projects::project_assign_document,
            projects::project_remove_document,
            // DEC-019：Rig Spike/MCP 命令保留为 Rust 测试资产，不注册进产品 IPC。
            agent::commands::agent_thread_create, // AG-13 RunStore：创建 Thread
            agent::commands::agent_thread_list,   // AG-13 RunStore：列出 Thread
            agent::commands::agent_thread_close,
            agent::commands::agent_thread_reopen,
            agent::commands::agent_thread_archive,
            agent::commands::agent_thread_pin,
            agent::commands::agent_collection_list,
            agent::commands::agent_collection_create,
            agent::commands::agent_thread_set_collection,
            agent::commands::agent_thread_gc,
            agent::commands::agent_thread_messages, // AG-13 RunStore：获取 Thread 消息
            agent::commands::agent_run_events_replay, // AG-13 RunStore：seq 重放
            agent::commands::agent_run_delete,      // AG-13 RunStore：级联删除 Run
            agent::commands::agent_run_start, // AG-14 Phase 2：启动 Agent 运行（事件同时写库+Channel）
            agent::commands::agent_thread_history, // AG-17 窗口重挂载恢复：Thread 全量事件史（含 seq=0，跨 Run 升序）
            agent::commands::agent_run_cancel,     // AG-18 运行取消：全局 CancellationToken 注册表
            agent::commands::agent_run_approval_respond, // Hermes 原生审批回传
            agent::commands::agent_run_clarify_respond, // Hermes 原生澄清回传
            agent::commands::agent_hermes_skills,  // Hermes Runtime 原生 Skill 目录
            agent::commands::agent_hermes_models,  // Hermes Runtime 原生模型/Provider 目录
            agent::commands::agent_hermes_model_catalog, // 设置页 Runtime 完整发现目录
            agent::commands::agent_hermes_usage,   // Hermes Runtime 精确 Token/调用用量
            agent::commands::agent_hermes_capabilities, // Hermes Runtime Skill/Tool/MCP/Browser 快照
            agent::commands::restart_hermes_runtime,    // 手动重启 Hermes Runtime（前端重连）
            agent::commands::agent_hermes_session_surface, // 会话占用条与 YOLO 状态
            agent::commands::agent_hermes_session_set_yolo, // 本轮 Hermes YOLO
            agent::commands::agent_hermes_session_slash, // /undo 等不建 Run 的 slash
            agent::commands::agent_hermes_cron_jobs,    // Hermes Runtime 计划任务与执行状态
            agent::commands::agent_hermes_cron_create,  // Hermes Runtime 计划任务创建
            agent::commands::agent_hermes_cron_update,  // Hermes Runtime 计划任务编辑
            agent::commands::agent_hermes_cron_set_enabled, // Hermes Runtime 计划任务启停
            agent::commands::agent_hermes_cron_trigger, // Hermes Runtime 计划任务立即触发
            agent::commands::agent_hermes_cron_runs,    // Hermes Runtime 计划任务运行历史
            agent::commands::agent_hermes_cron_run_result, // Hermes Runtime 计划任务单次结果
            agent::commands::agent_hermes_cron_delete,  // Hermes Runtime 计划任务删除
            agent::commands::agent_hermes_toolset_set_enabled, // Hermes Runtime Toolset 开关
            agent::commands::agent_hermes_skill_set_enabled, // Hermes Runtime Skill 开关
            agent::commands::agent_hermes_skill_document, // Hermes Runtime Skill 正文
            agent::commands::agent_hermes_skill_document_save, // Hermes Runtime Skill 编辑
            agent::commands::agent_hermes_skill_archive, // Hermes Runtime Skill 归档
            agent::commands::agent_hermes_terminal_backend_select, // Hermes Terminal 执行后端
            agent::commands::agent_hermes_skills_hub,   // Hermes Runtime Skills Hub 浏览/搜索
            agent::commands::agent_hermes_skill_hub_preview, // Hermes Runtime Skills Hub 预览
            agent::commands::agent_hermes_skill_install, // Hermes Runtime Skills Hub 安装/重载
            agent::commands::agent_hermes_browser_manage, // Hermes Runtime Browser 连接管理
            agent::commands::agent_hermes_mcp_add,      // Hermes Runtime MCP 新增/探测
            agent::commands::agent_hermes_mcp_set_enabled, // Hermes Runtime MCP 启停
            agent::commands::agent_hermes_mcp_test,     // Hermes Runtime MCP 连接探测
            agent::commands::agent_hermes_mcp_remove,   // Hermes Runtime MCP 移除
            agent::commands::agent_hermes_mcp_oauth_start, // Hermes Runtime MCP OAuth 发起
            agent::commands::agent_hermes_mcp_oauth_status, // Hermes Runtime MCP OAuth 轮询
            agent::commands::agent_hermes_mcp_reload,   // Hermes Runtime MCP 动态重载
            agent::commands::agent_hermes_mcp_catalog,  // Hermes Runtime MCP Catalog
            agent::commands::agent_hermes_mcp_catalog_install, // Hermes Runtime MCP Catalog 安装
            agent::commands::agent_run_snapshot, // AG-20 Run 状态快照（缺口补齐阶梯升级项：replay 填不上时全量重同步）
            agent::commands::agent_run_reconcile, // ISSUE-019：重开非终态会话时向 Hermes reattach，再按权威状态收口
            completion::completion_suggest,       // AG-30 自然语言补全（轻量路径，不建 Thread/Run）
            completion::completion_cancel,        // AG-30 补全取消传播
            completion::completion_metrics,       // AG-30 聚合指标（§4.5，只记聚合不记内容）
            completion::completion_report_feedback, // AG-30 接受/拒绝反馈计数
            documents::commands::document_preview_patch, // AG-24 修改提案 dry-run 预览（不写文件）
            documents::commands::document_apply_patch, // AG-24/26 批准应用提案（可逐 hunk 部分批准）
            documents::commands::document_reject_patch, // AG-24 拒绝提案
            documents::commands::document_undo,        // AG-24 撤销最近一次修订（可再撤销 = redo）
            documents::commands::document_undo_patch,  // operation 级 Agent checkpoint 撤销
            documents::commands::document_project_patches, // AG-26 项目 patch 列表（重启后重建审批卡/审计）
            documents::commands::document_current_version, // AG-26 文档当前版本号（选区 chip baseVersion）
            skills::skill_list, // AG-27 Phase 4：Skill 列表（三层来源 + 启用态 + 工具交集）
            skills::skill_set_enabled, // AG-27 Skill 启用/停用（安装 = 清单文件入 user/workspace 目录）
            knowledge::knowledge_version_status, // NEXT-053 KB-0 版本证据开关状态
            knowledge::knowledge_version_set_enabled,
            knowledge::knowledge_version_preview_baseline,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // 退出取证：把每次退出事件记进 dev.log——
    // ExitRequested（收到退出请求，code 可区分正常退出码）/ Exit（进程即将退出）。
    // 判定规则：日志中有退出标记 = 正常退出或被请求退出；
    // 有 PANIC 标记 = 进程内 panic；两者皆无且日志戛然而止 = 进程被异常杀死（崩溃/系统终止），
    // 此时去 Console.app → 崩溃报告 查 sophonote 的 .ips 报告
    app.run(|app_handle, event| match event {
        tauri::RunEvent::ExitRequested { code, .. } => {
            println!("[sophonote] exit requested: code={:?}", code);
        }
        tauri::RunEvent::Exit => {
            // 停止健康监督器后台轮询
            if let Ok(mut guard) = app_handle.state::<AppState>().hermes_health.lock() {
                if let Some(supervisor) = guard.take() {
                    supervisor.stop();
                }
            }
            if let Ok(mut runtime) = app_handle.state::<AppState>().hermes.try_lock() {
                if let Some(mut child) = runtime.take() {
                    child.shutdown();
                }
            }
            println!("[sophonote] process exiting");
        }
        _ => {}
    });
}
