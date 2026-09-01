export interface Item {
  id: string;
  sourceId: string;
  type: 'repo' | 'paper' | 'product' | 'article' | 'model';
  title: string;
  url: string;
  description: string;
  author?: string;
  language?: string;
  stars?: number;
  forks?: number;
  topics?: string[];
  publishedAt: string;
  fetchedAt: string;
  status: 'unread' | 'read' | 'archived' | 'starred';
  userNotes?: string;
  aiSummary?: string;
  aiTags?: string[];
  // 列表轻量标识（P0-5）：item_contents 联表带出
  contentStatus?: 'pending' | 'fetching' | 'ready' | 'partial' | 'failed' | 'unsupported';
  qualityLevel?: number;
}

export interface Source {
  id: string;
  name: string;
  type: 'github' | 'arxiv' | 'hackernews' | 'producthunt' | 'huggingface' | 'huggingface_papers' | 'aihot' | 'modelscope' | 'custom';
  enabled: boolean;
  config: Record<string, unknown>;
  fetchIntervalMinutes: number;
  lastFetchedAt?: string;
  createdAt: string;
  // 信源分层（借鉴 ai-news-radar source_tier）
  tier: 'core' | 'standard' | 'experimental';
  // 准入状态：active 正式 | probation 试用观察期（参与抓取但不进默认视图）| skipped 高风险跳过
  admission: 'active' | 'probation' | 'skipped';
  // 源健康三件套
  lastSuccessAt?: string | null;
  lastError?: string | null;
  fetchSuccessCount: number;
  fetchFailCount: number;
}

export interface SourceDiscoveryConfig {
  generationPrompt?: string;
  scoringRule?: string;
  minScore?: number;
}

/** 数据源联通状态：ok=最近一次抓取成功（绿）；error=最近一次失败（红）；idle=从未抓取（灰）。
 *  依据 scheduler::record_source_health 语义——成功时清空 last_error 并刷新 last_success_at。 */
export type SourceConnStatus = 'ok' | 'error' | 'idle';

export function sourceConnStatus(s: Pick<Source, 'lastError' | 'lastSuccessAt'>): SourceConnStatus {
  if (s.lastError) return 'error';
  if (s.lastSuccessAt) return 'ok';
  return 'idle';
}

export interface Collection {
  id: string;
  name: string;
  description?: string;
  icon?: string;
  color?: string;
  createdAt: string;
}

export interface DailyLog {
  id: string;
  date: string;
  type: 'daily' | 'weekly';
  content: string;
  sources?: string[];
  generatedBy: 'ai' | 'manual';
  createdAt: string;
  updatedAt?: string;
}

export interface Task {
  id: string;
  title: string;
  description?: string;
  status: 'todo' | 'in_progress' | 'done' | 'cancelled';
  priority: 1 | 2 | 3;
  dueDate?: string;
  recurring?: 'daily' | 'weekly' | 'none';
  tags?: string[];
  createdAt: string;
  completedAt?: string;
}

/** 番茄钟专注会话（DEC-034）：taskId 可空表示未关联任务的专注记录。 */
export interface PomodoroSession {
  id: string;
  taskId?: string;
  plannedMinutes: number;
  startedAt: string;
  endedAt?: string;
  completed: boolean;
}

// 单个供应商的独立配置（参考 cc-switch 模型：名称 + 协议 + 地址 + 模型）
export interface ProviderConfig {
  id: string;
  name: string;
  protocol: 'openai' | 'anthropic'; // 请求地址的接口格式
  baseUrl: string;
  model: string;
  models: string[];                 // 该供应商可选模型列表
  pricing?: string;                 // 计费说明
  /** false = 本地/私有化部署等无需 API Key 的端点（Ollama、vLLM 等）；缺省 true 表示必须配置凭据。 */
  requiresKey?: boolean;
}

// 向量嵌入配置（语义搜索用）：独立于对话模型，因 Deepseek/Kimi 均不提供 embeddings
export interface EmbeddingConfig {
  baseUrl: string;                  // openai 协议填服务根地址（自动拼 /embeddings）；dashscope 协议填完整服务地址
  model: string;                    // 如 BAAI/bge-m3、text-embedding-v4、qwen3.7-text-embedding
  protocol?: 'openai' | 'dashscope'; // 默认 openai；dashscope 为阿里 MaaS 原生格式（input.texts 数组）
}

// AI 配置中心：多套供应商配置 + 当前启用指针
export interface AIConfig {
  activeProvider: string;           // 当前启用的供应商 id
  providers: Record<string, ProviderConfig>;
  /** Composer 按供应商记住最近一次有效模型；模型被移除时回退 provider.model。 */
  lastAgentModelByProvider?: Record<string, string>;
  embedding?: EmbeddingConfig;      // 语义搜索嵌入模型（API Key 存钥匙串，key 为 "embedding"）
}

export interface Article {
  id: string;
  itemId?: string;
  title: string;
  content: string;
  articleType: 'deep-dive' | 'nightly' | 'manual' | 'journal';
  edited: boolean;
  createdAt: string;
  updatedAt?: string;
  promptVersion?: string | null;
  /** 已废弃：BlockSuite DocSnapshot JSON（块编辑模式已移除，仅历史数据保留，后端仍返回） */
  blocksJson?: string | null;
}

/** 发现页类别（LLM 打分 Top5 的分组维度） */
export type DiscoverCategory = 'github' | 'arxiv' | 'hackernews' | 'producthunt' | 'huggingface' | 'aihot';

/** 每日 Top5 入选记录（daily_picks 表 + 关联条目） */
export interface DailyPick {
  id: string;
  date: string;              // YYYY-MM-DD
  category: DiscoverCategory;
  rank: number;              // 1-5
  heatScore: number | null;  // 入选时热度快照（stars/upvotes/score/likes）
  aiScore: number | null;    // LLM 打分 0-10
  reason: string | null;     // LLM 推荐理由
  selectionLane: 'github' | 'model' | 'product';
  createdAt: string;
  item: Item;
}

/** 历史入选引用（跨天去重：热度增长超阈值视为版本迭代，允许再入选） */
export interface PickedRef {
  itemId: string;
  date: string;
  heatScore: number | null;
}

export interface AppSettings {
  theme: 'light' | 'dark' | 'system';
  language: 'zh' | 'en';
  autoFetch: boolean;
  fetchIntervalHours: number;
  notificationEnabled: boolean;
  aiConfig: AIConfig;
  semanticSearchEnabled: boolean;  // 语义搜索总开关：关闭时不读嵌入 Key、不显示语义模式
}

// ---- Track B · 智能体演进（AG-02 追加）：AI 工作室项目容器，见 docs/architecture.md ----
/** 项目 = 扁平分组容器（非标签）：一等实体、有成员名单、未来可承载项目内 Chat。
 *  parentId 扁平阶段恒为 null；有值即代表升级成文件夹树（schema 已预留，无迁移） */
export interface Project {
  id: string;
  name: string;
  description?: string | null;
  parentId?: string | null;
  /** 派生字段：成员文档数（服务端 project_list 子查询带出；前端以 memberships 实时派生为准） */
  docCount: number;
  /** 置顶项目优先显示 */
  pinned?: boolean;
  createdAt: string;
  updatedAt?: string | null;
}
