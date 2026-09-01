use chrono::Local;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio::time::interval;

use crate::db::{get_db_path, Item, Source};

/// Scheduler state
pub struct SchedulerState {
    pub last_fetch_check: Option<chrono::DateTime<Local>>,
    pub is_running: bool,
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self {
            last_fetch_check: None,
            is_running: true,
        }
    }
}

pub type SchedulerHandle = Arc<Mutex<SchedulerState>>;

/// Start the background scheduler
pub fn start_scheduler(app: AppHandle) -> SchedulerHandle {
    let state = Arc::new(Mutex::new(SchedulerState::default()));
    let state_clone = state.clone();

    tauri::async_runtime::spawn(async move {
        println!("[scheduler] started (60s tick)");
        match cleanup_expired_items(&app) {
            Ok(count) if count > 0 => {
                println!("[scheduler] expired inbox items cleaned on startup: {count}");
                if let Err(e) = crate::content::rebuild_stories(&app) {
                    eprintln!("[scheduler] rebuild_stories after cleanup failed: {e}");
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!("[scheduler] startup inbox cleanup failed: {e}"),
        }
        let mut ticker = interval(Duration::from_secs(60));

        loop {
            ticker.tick().await;

            let guard = state_clone.lock().await;
            if !guard.is_running {
                break;
            }
            drop(guard);

            let now = Local::now();
            let app_handle = app.clone();

            if let Err(e) = check_and_fetch_sources(&app_handle).await {
                eprintln!("[scheduler] Fetch check error: {}", e);
            }

            let mut guard = state_clone.lock().await;
            guard.last_fetch_check = Some(now);
        }
    });

    state
}

/// 单个来源的抓取结果（手动刷新与定时调度共用同一出口）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFetchResult {
    pub source_id: String,
    pub success: bool,
    pub fetched: usize,
    pub new_items: usize,
    #[serde(default)]
    pub new_item_ids: Vec<String>,
    pub error: Option<String>,
}

/// 无论成败都写入 last_fetched_at，避免失败源（如 401）因时间戳为空而每个 60s tick 重试。
fn touch_last_fetched_at(app: &AppHandle, source_id: &str) {
    let db_path = get_db_path(app);
    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
        let _ = conn.execute(
            "UPDATE sources SET last_fetched_at = datetime('now') WHERE id = ?1",
            rusqlite::params![source_id],
        );
    }
}

fn source_due_for_fetch(
    last_fetched_at: Option<&str>,
    interval_minutes: i32,
    now: chrono::DateTime<Local>,
) -> bool {
    match last_fetched_at {
        Some(last) => {
            let last_dt = chrono::DateTime::parse_from_rfc3339(last)
                .or_else(|_| {
                    chrono::NaiveDateTime::parse_from_str(last, "%Y-%m-%d %H:%M:%S")
                        .map(|dt| dt.and_utc().fixed_offset())
                })
                .unwrap_or_else(|_| now.fixed_offset());
            let elapsed = now.signed_duration_since(last_dt.with_timezone(&chrono::Local));
            elapsed.num_minutes() >= i64::from(interval_minutes)
        }
        None => true,
    }
}

/// 记录源健康状态（借鉴 ai-news-radar source-status）：成功/失败计数、最后成功时间、最后错误（截断 500 字）
fn record_source_health(app: &AppHandle, source_id: &str, success: bool, error: Option<&str>) {
    let Ok(conn) = rusqlite::Connection::open(get_db_path(app)) else {
        return;
    };
    let r = if success {
        conn.execute(
            "UPDATE sources SET fetch_success_count = fetch_success_count + 1, last_success_at = datetime('now'), last_error = NULL WHERE id = ?1",
            rusqlite::params![source_id],
        )
    } else {
        let msg: String = error.unwrap_or_default().chars().take(500).collect();
        conn.execute(
            "UPDATE sources SET fetch_fail_count = fetch_fail_count + 1, last_error = ?2 WHERE id = ?1",
            rusqlite::params![source_id, msg],
        )
    };
    if let Err(e) = r {
        eprintln!("[scheduler] record_source_health failed: {}", e);
    }
}

/// 抓取并保存一个来源：fetch → 标准化 → 去重写库 → 更新抓取时间 → 发事件
pub async fn fetch_and_save_source(app: &AppHandle, source: &Source) -> SourceFetchResult {
    println!("[scheduler] Fetching source: {}", source.id);
    match fetch_source_data(app, source).await {
        Ok(items) => match save_items_sync(app, &items) {
            Ok(new_ids) => {
                let new_count = new_ids.len();
                record_source_health(app, &source.id, true, None);
                touch_last_fetched_at(app, &source.id);
                let _ = app.emit(
                    "sophonote:fetch-completed",
                    serde_json::json!({
                        "sourceId": source.id,
                        "count": items.len(),
                    }),
                );
                // 内容预热：后台异步填充 item_contents，打开详情时秒开。
                // 每个源抓取后：新条目（≤20）+ 重试该源此前失败的条目（≤5），条间隔 500ms 防触发 GitHub 二级限流
                {
                    let app_clone = app.clone();
                    let source_id = source.id.clone();
                    let mut warm_ids: Vec<String> = new_ids.iter().take(20).cloned().collect();
                    if let Ok(conn) = rusqlite::Connection::open(get_db_path(app)) {
                        if let Ok(mut stmt) = conn.prepare(
                            "SELECT c.item_id FROM item_contents c JOIN items i ON i.id = c.item_id
                             WHERE c.status = 'failed' AND i.source_id = ?1 LIMIT 5",
                        ) {
                            let failed_ids: Vec<String> = stmt
                                .query_map(rusqlite::params![source_id], |row| {
                                    row.get::<_, String>(0)
                                })
                                .map(|iter| iter.filter_map(|r| r.ok()).collect())
                                .unwrap_or_default();
                            warm_ids.extend(failed_ids);
                        }
                    }
                    if !warm_ids.is_empty() {
                        tauri::async_runtime::spawn(async move {
                            for id in &warm_ids {
                                if let Err(e) =
                                    crate::content::get_or_fetch_item_content(&app_clone, id).await
                                {
                                    eprintln!(
                                        "[scheduler] Content warm-up failed for {}: {}",
                                        id, e
                                    );
                                }
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                            println!(
                                "[scheduler] Content warm-up done ({} items)",
                                warm_ids.len()
                            );
                        });
                    }
                }
                // 原始抓取数量只用于源健康与内部统计，不向用户推送。
                // 发现 Skill 完成过滤、生成和落库后，再由 Hermes Cron 的终态结果统一通知。
                SourceFetchResult {
                    source_id: source.id.clone(),
                    success: true,
                    fetched: items.len(),
                    new_items: new_count,
                    new_item_ids: new_ids,
                    error: None,
                }
            }
            Err(e) => {
                eprintln!("[scheduler] Save items error: {}", e);
                record_source_health(app, &source.id, false, Some(&e));
                touch_last_fetched_at(app, &source.id);
                SourceFetchResult {
                    source_id: source.id.clone(),
                    success: false,
                    fetched: 0,
                    new_items: 0,
                    new_item_ids: Vec::new(),
                    error: Some(e),
                }
            }
        },
        Err(e) => {
            eprintln!("[scheduler] Fetch {} failed: {}", source.id, e);
            record_source_health(app, &source.id, false, Some(&e));
            touch_last_fetched_at(app, &source.id);
            SourceFetchResult {
                source_id: source.id.clone(),
                success: false,
                fetched: 0,
                new_items: 0,
                new_item_ids: Vec::new(),
                error: Some(e),
            }
        }
    }
}

/// 从数据库加载启用中的来源（可按 id 过滤；admission='skipped' 的高风险源始终跳过）
fn load_enabled_sources(
    app: &AppHandle,
    source_ids: &Option<Vec<String>>,
) -> Result<Vec<Source>, String> {
    let db_path = get_db_path(app);
    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare("SELECT id, name, source_type, enabled, config, fetch_interval_minutes, last_fetched_at, tier, admission, last_success_at, last_error, fetch_success_count, fetch_fail_count FROM sources WHERE enabled = 1 AND admission != 'skipped'")
        .map_err(|e| e.to_string())?;

    let collected: Vec<Source> = stmt
        .query_map([], |row| {
            Ok(Source {
                id: row.get(0)?,
                name: row.get(1)?,
                source_type: row.get(2)?,
                enabled: row.get(3)?,
                config: row.get(4)?,
                fetch_interval_minutes: row.get(5)?,
                last_fetched_at: row.get(6)?,
                tier: row.get(7).unwrap_or_else(|_| "core".to_string()),
                admission: row.get(8).unwrap_or_else(|_| "active".to_string()),
                last_success_at: row.get(9).ok(),
                last_error: row.get(10).ok(),
                fetch_success_count: row.get(11).unwrap_or(0),
                fetch_fail_count: row.get(12).unwrap_or(0),
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .filter(|s| {
            source_ids
                .as_ref()
                .map(|ids| ids.contains(&s.id))
                .unwrap_or(true)
        })
        .collect();

    Ok(collected)
}

/// 统一抓取入口：手动刷新（fetch_sources_now 命令）与定时调度共用
pub async fn fetch_sources(
    app: &AppHandle,
    source_ids: Option<Vec<String>>,
) -> Vec<SourceFetchResult> {
    let sources = match load_enabled_sources(app, &source_ids) {
        Ok(s) => s,
        Err(e) => {
            return vec![SourceFetchResult {
                source_id: "*".to_string(),
                success: false,
                fetched: 0,
                new_items: 0,
                new_item_ids: Vec::new(),
                error: Some(e),
            }];
        }
    };

    let mut results = Vec::with_capacity(sources.len());
    for source in &sources {
        results.push(fetch_and_save_source(app, source).await);
    }

    if let Err(e) = cleanup_expired_items(app) {
        eprintln!("[scheduler] inbox TTL cleanup error: {e}");
    }

    // 故事级合并（借鉴 ai-news-radar stories-merged）：每轮抓取后重建 24h 故事分组
    match crate::content::rebuild_stories(app) {
        Ok(v) => println!("[scheduler] stories rebuilt: {}", v),
        Err(e) => eprintln!("[scheduler] rebuild_stories error: {}", e),
    }

    results
}

/// Check each enabled source and fetch if interval has passed
async fn check_and_fetch_sources(app: &AppHandle) -> Result<(), String> {
    let sources = load_enabled_sources(app, &None)?;
    let now = Local::now();

    for source in sources {
        if source_due_for_fetch(
            source.last_fetched_at.as_deref(),
            source.fetch_interval_minutes,
            now,
        ) {
            fetch_and_save_source(app, &source).await;
        }
    }

    let cleaned = cleanup_expired_items(app)?;
    if cleaned > 0 {
        crate::content::rebuild_stories(app)?;
    }

    Ok(())
}

/// Fetch data from a specific source
async fn fetch_source_data(app: &AppHandle, source: &Source) -> Result<Vec<Item>, String> {
    match source.source_type.as_str() {
        "github" => fetch_github_trending().await,
        "arxiv" => fetch_arxiv_papers().await,
        "hackernews" => fetch_hackernews().await,
        "huggingface" => fetch_huggingface_models().await,
        "huggingface_papers" => fetch_huggingface_papers(app).await,
        "producthunt" => fetch_producthunt(app, source.config.as_deref()).await,
        "aihot" => fetch_aihot_selected(app, source.config.as_deref()).await,
        _ => Err(format!("Unknown source type: {}", source.source_type)),
    }
}

// ==================== HuggingFace（部分网络环境经 hf-mirror 镜像） ====================

const HF_MIRROR: &str = "https://hf-mirror.com";

/// HuggingFace 模型榜（按 likes 排序，覆盖大模型/小模型/数据集）
async fn fetch_huggingface_models() -> Result<Vec<Item>, String> {
    let client = reqwest::Client::new();
    let res = client
        .get(format!("{}/api/models", HF_MIRROR))
        .query(&[("sort", "likes"), ("direction", "-1"), ("limit", "20")])
        .header("User-Agent", "SophoNote-App")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("HuggingFace API error: {}", res.status()));
    }

    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let models = data.as_array().cloned().unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();

    let results: Vec<Item> = models
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            let likes = m["likes"].as_i64().map(|n| n as i32);
            let downloads = m["downloads"].as_i64().unwrap_or(0);
            let pipeline = m["pipeline_tag"].as_str().unwrap_or("");
            let tags: Vec<String> = m["tags"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .take(6)
                        .collect()
                })
                .unwrap_or_default();
            let author = id.split('/').next().map(|s| s.to_string());
            let description = if pipeline.is_empty() {
                format!("下载量 {}", downloads)
            } else {
                format!("{} · 下载量 {}", pipeline, downloads)
            };
            Some(Item {
                id: format!("hfm-{}", id.replace('/', "-")),
                source_id: "huggingface-models".to_string(),
                item_type: "model".to_string(),
                title: id.to_string(),
                url: format!("https://huggingface.co/{}", id),
                description,
                author,
                language: if pipeline.is_empty() {
                    None
                } else {
                    Some(pipeline.to_string())
                },
                stars: likes,
                forks: None,
                topics: if tags.is_empty() {
                    None
                } else {
                    Some(tags.join(","))
                },
                published_at: m["lastModified"].as_str().unwrap_or("").to_string(),
                fetched_at: now.clone(),
                status: "unread".to_string(),
                ai_summary: None,
                ai_tags: None,
                content_status: None,
                quality_level: None,
            })
        })
        .collect();

    Ok(results)
}

/// HuggingFace 每日论文（社区票选，附开源实现链接）
/// 与 arXiv 去重合并：同一篇论文若已有 arxiv- 条目，只更新其热度，不再生成重复条目
async fn fetch_huggingface_papers(app: &AppHandle) -> Result<Vec<Item>, String> {
    let client = reqwest::Client::new();
    let res = client
        .get(format!("{}/api/daily_papers", HF_MIRROR))
        .header("User-Agent", "SophoNote-App")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("HuggingFace papers API error: {}", res.status()));
    }

    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let papers = data.as_array().cloned().unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();
    let db_path = get_db_path(app);

    let mut results: Vec<Item> = Vec::new();
    for p in papers.iter().take(20) {
        let paper = &p["paper"];
        let Some(pid) = paper["id"].as_str() else {
            continue;
        };
        let Some(title) = paper["title"].as_str().or_else(|| p["title"].as_str()) else {
            continue;
        };
        let summary = paper["summary"]
            .as_str()
            .or_else(|| p["summary"].as_str())
            .unwrap_or("");
        let upvotes = paper["upvotes"].as_i64().map(|n| n as i32);

        // 去重：arXiv 条目已存在则仅回写热度
        let arxiv_id = format!("arxiv-{}", pid);
        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM items WHERE id = ?1 LIMIT 1",
                    rusqlite::params![arxiv_id],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            if exists {
                let _ = conn.execute(
                    "UPDATE items SET stars = MAX(COALESCE(stars, 0), ?2) WHERE id = ?1",
                    rusqlite::params![arxiv_id, upvotes.unwrap_or(0)],
                );
                continue;
            }
        }

        let authors: Vec<String> = paper["authors"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a["name"].as_str().map(|s| s.to_string()))
                    .take(3)
                    .collect()
            })
            .unwrap_or_default();
        results.push(Item {
            id: format!("hfp-{}", pid),
            source_id: "huggingface-papers".to_string(),
            item_type: "paper".to_string(),
            title: title.to_string(),
            url: format!("https://huggingface.co/papers/{}", pid),
            description: summary.to_string(),
            author: if authors.is_empty() {
                None
            } else {
                Some(authors.join(", "))
            },
            language: Some("en".to_string()),
            stars: upvotes,
            forks: None,
            topics: None,
            published_at: p["publishedAt"].as_str().unwrap_or("").to_string(),
            fetched_at: now.clone(),
            status: "unread".to_string(),
            ai_summary: None,
            ai_tags: None,
            content_status: None,
            quality_level: None,
        });
    }

    Ok(results)
}

// ==================== ProductHunt（GraphQL，token 存 settings + 进程缓存） ====================

async fn fetch_producthunt(app: &AppHandle, config: Option<&str>) -> Result<Vec<Item>, String> {
    // 优先 settings/进程缓存；发现 config 中遗留明文 token 时迁移至 settings 并清空
    let cached = crate::commands::get_cached_api_key(app, "producthunt").unwrap_or_default();
    let token = if !cached.is_empty() {
        cached
    } else {
        let legacy = config
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|v| v["token"].as_str().map(|s| s.to_string()))
            .filter(|t| !t.is_empty());
        match legacy {
            Some(t) => {
                if let Ok(conn) = rusqlite::Connection::open(get_db_path(app)) {
                    let _ = conn.execute(
                        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('apikey:producthunt', ?1, datetime('now'))",
                        rusqlite::params![t],
                    );
                    let _ = conn.execute(
                        "UPDATE sources SET config = NULL WHERE id = 'producthunt'",
                        [],
                    );
                    println!("[scheduler] ProductHunt token 已从 config 迁移至 settings");
                }
                crate::commands::set_cached_api_key("producthunt", &t);
                t
            }
            None => {
                return Err("ProductHunt 需要 developer token：设置 → 数据源 中填入（producthunt.com/v2/oauth/applications 申请）".to_string());
            }
        }
    };

    let query = r#"{"query":"query { posts(order: VOTES, first: 15) { nodes { id name tagline url votesCount createdAt topics { nodes { name } } } } }"}"#;
    let client = reqwest::Client::new();
    let res = client
        .post("https://api.producthunt.com/v2/api/graphql")
        .bearer_auth(&token)
        .header("Content-Type", "application/json")
        .body(query)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("ProductHunt API error: {}", res.status()));
    }

    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if let Some(errors) = data["errors"].as_array() {
        if !errors.is_empty() {
            return Err(format!(
                "ProductHunt GraphQL error: {}",
                errors[0]["message"].as_str().unwrap_or("unknown")
            ));
        }
    }
    let posts = data["data"]["posts"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();

    let results: Vec<Item> = posts
        .iter()
        .filter_map(|p| {
            let id = p["id"].as_str()?;
            let topics: Vec<String> = p["topics"]["nodes"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
                        .take(5)
                        .collect()
                })
                .unwrap_or_default();
            Some(Item {
                id: format!("ph-{}", id),
                source_id: "producthunt".to_string(),
                item_type: "product".to_string(),
                title: p["name"].as_str()?.to_string(),
                url: p["url"].as_str()?.to_string(),
                description: p["tagline"].as_str().unwrap_or("").to_string(),
                author: None,
                language: Some("en".to_string()),
                stars: p["votesCount"].as_i64().map(|n| n as i32),
                forks: None,
                topics: if topics.is_empty() {
                    None
                } else {
                    Some(topics.join(","))
                },
                published_at: p["createdAt"].as_str().unwrap_or("").to_string(),
                fetched_at: now.clone(),
                status: "unread".to_string(),
                ai_summary: None,
                ai_tags: None,
                content_status: None,
                quality_level: None,
            })
        })
        .collect();

    Ok(results)
}

/// Fetch GitHub trending repositories
async fn fetch_github_trending() -> Result<Vec<Item>, String> {
    let client = reqwest::Client::new();
    let res = client
        .get("https://api.github.com/search/repositories")
        .query(&[
            ("q", "stars:>1000"),
            ("sort", "updated"),
            ("order", "desc"),
            ("per_page", "20"),
        ])
        .header("User-Agent", "SophoNote-App")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("GitHub API error: {}", res.status()));
    }

    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let items = data["items"].as_array().cloned().unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();

    let results: Vec<Item> = items
        .iter()
        .filter_map(|repo| {
            let id = repo["id"].as_i64()?;
            Some(Item {
                id: format!("gh-{}", id),
                source_id: "github-trending".to_string(),
                item_type: "repo".to_string(),
                title: repo["full_name"].as_str()?.to_string(),
                url: repo["html_url"].as_str()?.to_string(),
                description: repo["description"].as_str().unwrap_or("").to_string(),
                author: Some(repo["owner"]["login"].as_str()?.to_string()),
                language: repo["language"].as_str().map(|s| s.to_string()),
                stars: repo["stargazers_count"].as_i64().map(|n| n as i32),
                forks: repo["forks_count"].as_i64().map(|n| n as i32),
                topics: repo["topics"].as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                        .join(",")
                }),
                published_at: repo["created_at"].as_str()?.to_string(),
                fetched_at: now.clone(),
                status: "unread".to_string(),
                ai_summary: None,
                ai_tags: None,
                content_status: None,
                quality_level: None,
            })
        })
        .collect();

    Ok(results)
}

/// Fetch arXiv AI papers
async fn fetch_arxiv_papers() -> Result<Vec<Item>, String> {
    let client = reqwest::Client::new();
    let url = "https://export.arxiv.org/api/query?search_query=cat:cs.AI+OR+cat:cs.CL+OR+cat:cs.LG&start=0&max_results=15&sortBy=submittedDate&sortOrder=descending";
    let mut last_error = String::from("arXiv 请求未执行");
    let mut xml = None;
    for attempt in 0..3 {
        match client
            .get(url)
            .header("User-Agent", "SophoNote/0.1 discovery@local")
            .timeout(Duration::from_secs(30))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                xml = Some(response.text().await.map_err(|error| error.to_string())?);
                break;
            }
            Ok(response) => last_error = format!("arXiv API error: {}", response.status()),
            Err(error) => last_error = error.to_string(),
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    }
    let xml = xml.ok_or(last_error)?;
    let entries = parse_arxiv_xml(&xml)?;
    let now = chrono::Utc::now().to_rfc3339();

    let results: Vec<Item> = entries
        .into_iter()
        .map(|entry| {
            let paper_id = entry
                .id
                .split('/')
                .next_back()
                .unwrap_or(&entry.id)
                .to_string();
            Item {
                id: format!("arxiv-{}", paper_id),
                source_id: "arxiv-ai".to_string(),
                item_type: "paper".to_string(),
                title: entry.title,
                url: entry.pdf_url,
                description: entry.summary,
                author: Some(entry.authors.join(", ")),
                language: Some("en".to_string()),
                stars: None,
                forks: None,
                topics: Some(entry.categories.join(",")),
                published_at: entry.published,
                fetched_at: now.clone(),
                status: "unread".to_string(),
                ai_summary: None,
                ai_tags: None,
                content_status: None,
                quality_level: None,
            }
        })
        .collect();

    Ok(results)
}

struct ArxivEntry {
    id: String,
    title: String,
    summary: String,
    authors: Vec<String>,
    published: String,
    categories: Vec<String>,
    pdf_url: String,
}

fn parse_arxiv_xml(xml: &str) -> Result<Vec<ArxivEntry>, String> {
    use std::sync::OnceLock;
    // AG-23（审计 P1-3 性能项）：正则全部静态化。原实现每条 entry、每个字段都
    // 重新编译 Regex（条目数十条 × 字段 6 个 = 数百次编译）；模式为常量，
    // 静态缓存后行为完全等价。注意只用 get_or_init（宿主 rustc 旧，禁 get_or_try_init）
    static ENTRY_RE: OnceLock<regex::Regex> = OnceLock::new();
    static ID_RE: OnceLock<regex::Regex> = OnceLock::new();
    static TITLE_RE: OnceLock<regex::Regex> = OnceLock::new();
    static SUMMARY_RE: OnceLock<regex::Regex> = OnceLock::new();
    static PUBLISHED_RE: OnceLock<regex::Regex> = OnceLock::new();
    static NAME_RE: OnceLock<regex::Regex> = OnceLock::new();
    static PDF_RE: OnceLock<regex::Regex> = OnceLock::new();
    static ALT_RE: OnceLock<regex::Regex> = OnceLock::new();
    static CAT_RE: OnceLock<regex::Regex> = OnceLock::new();

    let entry_regex = ENTRY_RE
        .get_or_init(|| regex::Regex::new(r"<entry>([\s\S]*?)</entry>").expect("static regex"));
    // Clippy regex_creation_in_loops：初始化全部提到循环外（OnceLock 只编译一次，
    // 但构造调用词法位置在循环内也会被 lint 拒绝）
    let id_re =
        ID_RE.get_or_init(|| regex::Regex::new(r"<id[^>]*>([\s\S]*?)</id>").expect("static regex"));
    let title_re = TITLE_RE.get_or_init(|| {
        regex::Regex::new(r"<title[^>]*>([\s\S]*?)</title>").expect("static regex")
    });
    let summary_re = SUMMARY_RE.get_or_init(|| {
        regex::Regex::new(r"<summary[^>]*>([\s\S]*?)</summary>").expect("static regex")
    });
    let published_re = PUBLISHED_RE.get_or_init(|| {
        regex::Regex::new(r"<published[^>]*>([\s\S]*?)</published>").expect("static regex")
    });
    let name_re = NAME_RE
        .get_or_init(|| regex::Regex::new(r"<name>([\s\S]*?)</name>").expect("static regex"));
    let pdf_re = PDF_RE.get_or_init(|| {
        regex::Regex::new(r#"<link[^>]*href="([^"]+)"[^>]*title="pdf"[^>]*/>"#)
            .expect("static regex")
    });
    let alt_re = ALT_RE.get_or_init(|| {
        regex::Regex::new(r#"<link[^>]*href="([^"]+)"[^>]*rel="alternate"[^>]*/>"#)
            .expect("static regex")
    });
    let cat_re = CAT_RE.get_or_init(|| {
        regex::Regex::new(r#"<category[^>]*term="([^"]+)"[^>]*/>"#).expect("static regex")
    });

    let mut entries = Vec::new();
    for cap in entry_regex.captures_iter(xml) {
        let entry_xml = cap.get(1).map(|m| m.as_str()).unwrap_or("");

        let get_text = |re: &'static regex::Regex| -> String {
            re.captures(entry_xml)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().replace('\n', " "))
                .unwrap_or_default()
        };

        let authors: Vec<String> = name_re
            .captures_iter(entry_xml)
            .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
            .collect();

        let pdf_url = pdf_re
            .captures(entry_xml)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| {
                alt_re
                    .captures(entry_xml)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default()
            });

        let id = get_text(id_re);
        let id_clean = id.split('/').next_back().unwrap_or(&id).to_string();

        entries.push(ArxivEntry {
            id: id_clean,
            title: get_text(title_re).replace('\n', " "),
            summary: get_text(summary_re).replace('\n', " "),
            authors,
            published: get_text(published_re),
            categories: cat_re
                .captures_iter(entry_xml)
                .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                .collect(),
            pdf_url,
        });
    }

    Ok(entries)
}

/// Fetch HackerNews top stories
async fn fetch_hackernews() -> Result<Vec<Item>, String> {
    let client = reqwest::Client::new();

    let ids_res = client
        .get("https://hacker-news.firebaseio.com/v0/topstories.json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let ids: Vec<i64> = ids_res.json().await.map_err(|e| e.to_string())?;
    let top_ids: Vec<i64> = ids.into_iter().take(15).collect();
    let now = chrono::Utc::now().to_rfc3339();

    let mut results = Vec::new();

    for id in top_ids {
        let story_res = client
            .get(format!(
                "https://hacker-news.firebaseio.com/v0/item/{}.json",
                id
            ))
            .timeout(Duration::from_secs(10))
            .send()
            .await;

        if let Ok(res) = story_res {
            if let Ok(story) = res.json::<serde_json::Value>().await {
                if let Some(title) = story["title"].as_str() {
                    let url = story["url"]
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={}", id));

                    results.push(Item {
                        id: format!("hn-{}", id),
                        source_id: "hackernews".to_string(),
                        item_type: "article".to_string(),
                        title: title.to_string(),
                        url,
                        description: story["text"].as_str().unwrap_or("").to_string(),
                        author: story["by"].as_str().map(|s| s.to_string()),
                        language: Some("en".to_string()),
                        stars: story["score"].as_i64().map(|n| n as i32),
                        forks: None,
                        topics: None,
                        published_at: story["time"]
                            .as_i64()
                            .map(|t| {
                                chrono::DateTime::from_timestamp(t, 0)
                                    .map(|dt| dt.to_rfc3339())
                                    .unwrap_or_default()
                            })
                            .unwrap_or_else(|| now.clone()),
                        fetched_at: now.clone(),
                        status: "unread".to_string(),
                        ai_summary: None,
                        ai_tags: None,
                        content_status: None,
                        quality_level: None,
                    });
                }
            }
        }
    }

    Ok(results)
}

// ==================== AIHOT（官方匿名只读 v1 API；个人非商业免费） ====================

const AIHOT_BASE: &str = "https://aihot.virxact.com";

/// AIHOT category → item_type（见 references/aihot-source.md 入库映射契约）
fn aihot_item_type(category: Option<&str>) -> &'static str {
    match category {
        Some("ai-models") => "model",
        Some("paper") => "paper",
        Some("ai-products") => "product",
        _ => "article",
    }
}

/// 纯映射：AIHOT items API 单条 JSON → SophoNote Item。与网络 I/O 解耦以便单测。
/// 必有字段（id / title / links.aihot / discoveredAt）缺失即丢弃；可空字段按合同回退。
fn map_aihot_item(value: &serde_json::Value) -> Option<Item> {
    let id = value["id"].as_str()?;
    let title = value["title"].as_str()?;
    let aihot_link = value["links"]["aihot"].as_str()?;
    let discovered_at = value["discoveredAt"].as_str()?.to_string();
    let url = value["links"]["original"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| aihot_link.to_string());
    let summary = value["summary"].as_str().unwrap_or_default();
    let description = value["originalTitle"]
        .as_str()
        .filter(|t| !t.trim().is_empty() && *t != title)
        .map(|t| format!("原标题：{t}\n{summary}"))
        .unwrap_or_else(|| summary.to_string());
    let published_at = value["publishedAt"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| discovered_at.clone());
    let category = value["category"].as_str();
    Some(Item {
        id: format!("aihot-{id}"),
        source_id: "aihot".to_string(),
        item_type: aihot_item_type(category).to_string(),
        title: title.to_string(),
        url,
        description,
        author: value["source"]["name"].as_str().map(|s| s.to_string()),
        language: Some("zh".to_string()),
        // AIHOT score（0-100）仅作源内候选排序热度；我方打分趟会按 0-10 口径重打
        stars: value["score"].as_i64().map(|n| n as i32),
        forks: None,
        // category 以 aihot:<slug> 留档作 aspect 提示，不进入受控主题词表
        topics: category.map(|c| format!("aihot:{c}")),
        published_at,
        fetched_at: discovered_at,
        status: "unread".to_string(),
        ai_summary: None,
        ai_tags: None,
        content_status: None,
        quality_level: None,
    })
}

/// 从 sources.config JSON 读 ETag（AIHOT API 合同约定 If-None-Match 增量）
fn aihot_etag_from_config(raw_config: Option<&str>) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(raw_config?)
        .ok()?
        .get("aihotEtag")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 把 ETag 合并回 sources.config JSON（保留其它键，如用户自定义生成规则）
fn aihot_config_with_etag(raw_config: Option<&str>, etag: &str) -> String {
    let mut map = raw_config
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    map.insert(
        "aihotEtag".to_string(),
        serde_json::Value::String(etag.to_string()),
    );
    serde_json::Value::Object(map).to_string()
}

/// AIHOT 精选池：官方 v1 API，匿名只读、无需 Key。
/// 合同：mode=selected + window=24h；ETag/304 增量；定时调用间隔 ≥60 秒；
/// 用途限个人非商业/公益非商业/组织内部（SophoNote 本地入库属个人使用）。
async fn fetch_aihot_selected(app: &AppHandle, config: Option<&str>) -> Result<Vec<Item>, String> {
    let mut request = reqwest::Client::new()
        .get(format!("{AIHOT_BASE}/api/v1/items"))
        .query(&[("mode", "selected"), ("window", "24h"), ("limit", "50")])
        .header(
            "User-Agent",
            "SophoNote/1.0 (aihot v1 client; personal non-commercial)",
        )
        .timeout(Duration::from_secs(20));
    if let Some(etag) = aihot_etag_from_config(config) {
        request = request.header("If-None-Match", etag);
    }
    let res = request.send().await.map_err(|e| e.to_string())?;

    if res.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Vec::new());
    }
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        let problem = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("title")
                    .or_else(|| v.get("detail"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| body.chars().take(160).collect());
        return Err(format!("AIHOT API 错误 {status}：{problem}"));
    }

    let etag = res
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let payload: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let results: Vec<Item> = payload["items"]
        .as_array()
        .map(|items| items.iter().filter_map(map_aihot_item).collect())
        .unwrap_or_default();

    if let Some(etag) = etag {
        if let Ok(conn) = rusqlite::Connection::open(get_db_path(app)) {
            let merged = aihot_config_with_etag(config, &etag);
            let _ = conn.execute(
                "UPDATE sources SET config = ?1 WHERE id = 'aihot'",
                rusqlite::params![merged],
            );
        }
    }
    Ok(results)
}

/// Save items to database (sync version - no await needed).
/// `inbox_item_ttl` 是跨清理保留的极小型账本：同一稳定 id 即使正文已过期删除，
/// 后续重复抓取也不能获得新的 7 天 TTL。
fn save_items_with_conn(
    conn: &mut rusqlite::Connection,
    items: &[Item],
) -> Result<Vec<String>, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut new_ids = Vec::new();
    for item in items {
        tx.execute(
            "INSERT INTO inbox_item_ttl (item_id, first_fetched_at, last_seen_at, expires_at)
             VALUES (?1, datetime('now'), datetime('now'), datetime('now', '+168 hours'))
             ON CONFLICT(item_id) DO UPDATE SET last_seen_at = datetime('now')",
            rusqlite::params![item.id],
        )
        .map_err(|e| e.to_string())?;

        let (first_fetched_at, last_seen_at, expires_at, active): (String, String, String, bool) =
            tx.query_row(
                "SELECT first_fetched_at, last_seen_at, expires_at,
                        datetime(expires_at) > datetime('now')
                 FROM inbox_item_ttl WHERE item_id = ?1",
                rusqlite::params![item.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|e| e.to_string())?;
        if !active {
            continue;
        }

        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM items WHERE id = ?1)",
                rusqlite::params![item.id],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !exists {
            new_ids.push(item.id.clone());
        }
        tx.execute(
            "INSERT INTO items (id, source_id, item_type, title, url, description, author, language, stars, forks, topics, published_at, fetched_at, first_fetched_at, last_seen_at, expires_at, status, ai_summary, ai_tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, datetime('now'), ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                url = excluded.url,
                description = excluded.description,
                author = excluded.author,
                language = excluded.language,
                stars = excluded.stars,
                forks = excluded.forks,
                topics = excluded.topics,
                published_at = excluded.published_at,
                fetched_at = datetime('now'),
                last_seen_at = excluded.last_seen_at",
            rusqlite::params![
                item.id, item.source_id, item.item_type, item.title, item.url,
                item.description, item.author, item.language, item.stars, item.forks,
                item.topics, item.published_at, first_fetched_at, last_seen_at, expires_at, item.status,
                item.ai_summary, item.ai_tags
            ],
        ).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(new_ids)
}

fn save_items_sync(app: &AppHandle, items: &[Item]) -> Result<Vec<String>, String> {
    let db_path = get_db_path(app);
    let mut conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    save_items_with_conn(&mut conn, items)
}

fn table_exists(conn: &rusqlite::Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        rusqlite::params![table],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

/// 删除已过 168 小时的收件箱原始数据与派生缓存。
/// `articles` 是已沉淀的 Markdown 真相，只解除来源关系；`inbox_item_ttl` 保留用于防续期。
fn cleanup_expired_items_with_conn(conn: &mut rusqlite::Connection) -> Result<usize, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let expired: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM items
             WHERE datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) <= datetime('now')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if expired == 0 {
        tx.commit().map_err(|e| e.to_string())?;
        return Ok(0);
    }

    const EXPIRED_ITEMS: &str =
        "SELECT id FROM items WHERE datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) <= datetime('now')";
    tx.execute(
        &format!("UPDATE articles SET item_id = NULL WHERE item_id IN ({EXPIRED_ITEMS})"),
        [],
    )
    .map_err(|e| e.to_string())?;
    for table in [
        "daily_picks",
        "collection_items",
        "item_contents",
        "item_chunks",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE item_id IN ({EXPIRED_ITEMS})"),
            [],
        )
        .map_err(|e| e.to_string())?;
    }
    for table in ["vec_items", "vec_chunks"] {
        if table_exists(&tx, table) {
            tx.execute(
                &format!("DELETE FROM {table} WHERE item_id IN ({EXPIRED_ITEMS})"),
                [],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    tx.execute(
        "DELETE FROM items
         WHERE datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) <= datetime('now')",
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(expired as usize)
}

pub fn cleanup_expired_items(app: &AppHandle) -> Result<usize, String> {
    let mut conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    cleanup_expired_items_with_conn(&mut conn)
}

#[cfg(test)]
mod fetch_interval_tests {
    use super::source_due_for_fetch;
    use chrono::{Duration, Local, Utc};

    #[test]
    fn never_fetched_source_is_due() {
        assert!(source_due_for_fetch(None, 360, Local::now()));
    }

    #[test]
    fn recent_failure_respects_interval() {
        let now = Local::now();
        let stamped = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        assert!(
            !source_due_for_fetch(Some(&stamped), 360, now),
            "失败后写入 last_fetched_at 不得在间隔内重试"
        );
    }

    #[test]
    fn elapsed_interval_is_due() {
        let now = Local::now();
        let old = (Utc::now() - Duration::minutes(361))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert!(source_due_for_fetch(Some(&old), 360, now));
    }
}

#[cfg(test)]
mod aihot_tests {
    use super::*;
    use serde_json::json;

    fn inbox_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("open db");
        crate::db::create_schema(&conn).expect("create schema");
        conn.execute(
            "INSERT INTO sources (id, name, source_type) VALUES ('test-source', 'Test', 'custom')",
            [],
        )
        .expect("insert source");
        conn
    }

    fn inbox_item(id: &str) -> Item {
        Item {
            id: id.to_string(),
            source_id: "test-source".to_string(),
            item_type: "article".to_string(),
            title: "First title".to_string(),
            url: "https://example.com/item".to_string(),
            description: "description".to_string(),
            author: None,
            language: Some("en".to_string()),
            stars: None,
            forks: None,
            topics: None,
            published_at: "2026-08-19T00:00:00Z".to_string(),
            fetched_at: "2026-08-19T00:00:00Z".to_string(),
            status: "unread".to_string(),
            ai_summary: None,
            ai_tags: None,
            content_status: None,
            quality_level: None,
        }
    }

    #[test]
    fn duplicate_fetch_updates_last_seen_without_renewing_ttl() {
        let mut conn = inbox_db();
        save_items_with_conn(&mut conn, &[inbox_item("ttl-1")]).expect("first save");
        let original: (String, String) = conn
            .query_row(
                "SELECT first_fetched_at, expires_at FROM inbox_item_ttl WHERE item_id = 'ttl-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read ttl");
        conn.execute(
            "UPDATE inbox_item_ttl SET last_seen_at = datetime('now', '-1 hour') WHERE item_id = 'ttl-1'",
            [],
        )
        .expect("age last seen");
        let old_last_seen: String = conn
            .query_row(
                "SELECT last_seen_at FROM inbox_item_ttl WHERE item_id = 'ttl-1'",
                [],
                |row| row.get(0),
            )
            .expect("read last seen");

        let mut updated = inbox_item("ttl-1");
        updated.title = "Updated title".to_string();
        assert!(save_items_with_conn(&mut conn, &[updated])
            .expect("duplicate save")
            .is_empty());

        let current: (String, String, String, String) = conn
            .query_row(
                "SELECT t.first_fetched_at, t.last_seen_at, t.expires_at, i.title
                 FROM inbox_item_ttl t JOIN items i ON i.id = t.item_id WHERE t.item_id = 'ttl-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read updated ttl");
        assert_eq!(current.0, original.0);
        assert_eq!(current.2, original.1);
        assert!(current.1 > old_last_seen);
        assert_eq!(current.3, "Updated title");
    }

    #[test]
    fn cleanup_drops_expired_raw_data_but_preserves_markdown_and_tombstone() {
        let mut conn = inbox_db();
        save_items_with_conn(&mut conn, &[inbox_item("ttl-expired")]).expect("save item");
        conn.execute_batch(
            "UPDATE inbox_item_ttl SET expires_at = datetime('now', '-1 second') WHERE item_id = 'ttl-expired';
             UPDATE items SET expires_at = datetime('now', '-1 second') WHERE id = 'ttl-expired';
             INSERT INTO item_contents (item_id, status) VALUES ('ttl-expired', 'ready');
             INSERT INTO item_chunks (chunk_id, item_id, chunk_idx, text) VALUES ('chunk-1', 'ttl-expired', 0, 'body');
             INSERT INTO collections (id, name) VALUES ('saved', 'Saved');
             INSERT INTO collection_items (collection_id, item_id) VALUES ('saved', 'ttl-expired');
             INSERT INTO articles (id, item_id, title, content) VALUES ('note-1', 'ttl-expired', 'Settled note', '# kept');",
        )
        .expect("seed dependent data");

        assert_eq!(
            cleanup_expired_items_with_conn(&mut conn).expect("cleanup"),
            1
        );
        let item_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        let content_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM item_contents", [], |row| row.get(0))
            .unwrap();
        let chunk_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM item_chunks", [], |row| row.get(0))
            .unwrap();
        let collection_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM collection_items", [], |row| {
                row.get(0)
            })
            .unwrap();
        let article_item_id: Option<String> = conn
            .query_row(
                "SELECT item_id FROM articles WHERE id = 'note-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let ttl_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM inbox_item_ttl", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            (item_count, content_count, chunk_count, collection_count),
            (0, 0, 0, 0)
        );
        assert_eq!(article_item_id, None);
        assert_eq!(ttl_count, 1);

        assert!(
            save_items_with_conn(&mut conn, &[inbox_item("ttl-expired")])
                .expect("repeat expired item")
                .is_empty()
        );
        let item_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(item_count, 0, "重复抓取不得让过期条目重新续命");
    }

    fn sample_item() -> serde_json::Value {
        json!({
            "id": "abc123",
            "title": "GPT-6 发布",
            "originalTitle": "GPT-6 released",
            "summary": "OpenAI 发布新旗舰模型。",
            "source": { "name": "机器之心" },
            "links": {
                "aihot": "https://aihot.virxact.com/item/abc123",
                "original": "https://example.com/gpt6"
            },
            "publishedAt": "2026-08-17T01:00:00Z",
            "discoveredAt": "2026-08-17T02:30:00Z",
            "category": "ai-models",
            "score": 92,
            "selected": true
        })
    }

    #[test]
    fn maps_full_item() {
        let item = map_aihot_item(&sample_item()).expect("映射成功");
        assert_eq!(item.id, "aihot-abc123");
        assert_eq!(item.source_id, "aihot");
        assert_eq!(item.item_type, "model");
        assert_eq!(item.url, "https://example.com/gpt6");
        assert!(item.description.starts_with("原标题：GPT-6 released"));
        assert_eq!(item.author.as_deref(), Some("机器之心"));
        assert_eq!(item.language.as_deref(), Some("zh"));
        assert_eq!(item.stars, Some(92));
        assert_eq!(item.topics.as_deref(), Some("aihot:ai-models"));
        assert_eq!(item.published_at, "2026-08-17T01:00:00Z");
        assert_eq!(item.fetched_at, "2026-08-17T02:30:00Z");
    }

    #[test]
    fn falls_back_when_nullable_fields_missing() {
        let mut value = sample_item();
        value["links"]["original"] = json!(null);
        value["publishedAt"] = json!(null);
        value["originalTitle"] = json!(null);
        value["score"] = json!(null);
        value["category"] = json!(null);
        let item = map_aihot_item(&value).expect("可空字段缺失仍映射");
        assert_eq!(item.url, "https://aihot.virxact.com/item/abc123");
        assert_eq!(item.published_at, "2026-08-17T02:30:00Z");
        assert_eq!(item.description, "OpenAI 发布新旗舰模型。");
        assert_eq!(item.stars, None);
        assert_eq!(item.item_type, "article");
        assert_eq!(item.topics, None);
    }

    #[test]
    fn drops_item_missing_required_fields() {
        let mut value = sample_item();
        value["id"] = json!(null);
        assert!(map_aihot_item(&value).is_none());
        let mut value = sample_item();
        value["links"]["aihot"] = json!(null);
        assert!(map_aihot_item(&value).is_none());
    }

    #[test]
    fn item_type_follows_category_contract() {
        assert_eq!(aihot_item_type(Some("ai-models")), "model");
        assert_eq!(aihot_item_type(Some("paper")), "paper");
        assert_eq!(aihot_item_type(Some("ai-products")), "product");
        assert_eq!(aihot_item_type(Some("industry")), "article");
        assert_eq!(aihot_item_type(Some("tip")), "article");
        assert_eq!(aihot_item_type(None), "article");
    }

    #[test]
    fn etag_round_trips_through_config_json() {
        assert_eq!(aihot_etag_from_config(None), None);
        assert_eq!(aihot_etag_from_config(Some("not json")), None);
        let merged = aihot_config_with_etag(None, "\"v1-etag\"");
        assert_eq!(
            aihot_etag_from_config(Some(&merged)),
            Some("\"v1-etag\"".to_string())
        );
        // 合并 ETag 必须保留用户自定义规则键
        let merged = aihot_config_with_etag(Some(r#"{"minScore":9.0}"#), "e2");
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed["minScore"], json!(9.0));
        assert_eq!(parsed["aihotEtag"], json!("e2"));
    }
}
