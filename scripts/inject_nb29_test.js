#!/usr/bin/env node
/**
 * NB-29 测试笔记一键注入：往 SophoNote 数据库插入两篇笔记 + 创建 .md 文件。
 * 用法：先关闭 SophoNote（./scripts/sophonote.sh stop），再运行本脚本，最后启动。
 * 运行后重启 SophoNote 即可在笔记本中看到「技术调研」与「块引用测试」两篇笔记。
 */

const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const APP_DIR = path.join(
  process.env.HOME,
  'Library/Application Support/com.fei.sophonote'
);
const DB_PATH = path.join(APP_DIR, 'sophonote.db');
const NOTES_DIR = path.join(APP_DIR, 'notes');

if (!fs.existsSync(DB_PATH)) {
  console.error('❌ 找不到数据库:', DB_PATH);
  console.error('   请先启动过一次 SophoNote 让数据库初始化。');
  process.exit(1);
}

// 检查笔记是否已存在（幂等）
const existing = execSync(
  `sqlite3 "${DB_PATH}" "SELECT COUNT(*) FROM articles WHERE title IN ('技术调研', '块引用测试');"`,
  { encoding: 'utf-8' }
).trim();

if (existing === '2') {
  console.log('✅ 两篇测试笔记已存在，跳过。');
  process.exit(0);
}

const now = new Date().toISOString();
const id1 = crypto.randomUUID();
const id2 = crypto.randomUUID();

const note1 = `## 背景

这是一个关于知识管理的调研。

## 核心发现

SophoNote 采用 Tauri v2 架构，Rust 拥有所有副作用，React 前端只负责渲染与 invoke 调用。这个决策使得跨域问题在 Rust 侧彻底解决。 ^core-arch

## 竞品对比

| 产品 | 双链 | 块引用 | 嵌入 |
|---|---|---|---|
| Obsidian | ✅ | ✅ | ✅ |
| Logseq | ✅ | ✅ | ✅ |
| SophoNote | ✅ | ✅ | ✅ | ^compare-table

## 结论

双链生态对标 Obsidian 核心语法已齐备。
`;

const note2 = `# 块引用测试

## 跳转测试

点击这个链接会跳转到「技术调研」的对应段落：[[技术调研#^core-arch]]

再试一个：[[技术调研#^compare-table]]

## 嵌入测试

下面是嵌入的块内容（卡片内渲染）：

![[技术调研#^core-arch]]

嵌入表格块：

![[技术调研#^compare-table]]

## 混合对比

普通段落嵌入（标题段，包含整个章节）：
![[技术调研#核心发现]]

块嵌入（只取那一段）：
![[技术调研#^core-arch]]
`;

// 写入 .md 文件
function writeMd(id, title, content) {
  const frontmatter = `---\ntitle: ${title}\ncreated: ${now}\narticle_type: manual\n---\n\n`;
  const mdPath = path.join(NOTES_DIR, `${id}.md`);
  fs.writeFileSync(mdPath, frontmatter + content, 'utf-8');
  return mdPath;
}

// 插入数据库
function insertArticle(id, title, content) {
  // 转义单引号
  const safeTitle = title.replace(/'/g, "''");
  execSync(
    `sqlite3 "${DB_PATH}" "INSERT OR IGNORE INTO articles (id, title, content, article_type, created_at, blocks_json) VALUES ('${id}', '${safeTitle}', '', 'manual', '${now}', NULL);"`,
    { encoding: 'utf-8' }
  );
}

try {
  fs.mkdirSync(NOTES_DIR, { recursive: true });

  const p1 = writeMd(id1, '技术调研', note1);
  insertArticle(id1, '技术调研', note1);
  console.log(`✅ 创建笔记「技术调研」: ${p1}`);

  const p2 = writeMd(id2, '块引用测试', note2);
  insertArticle(id2, '块引用测试', note2);
  console.log(`✅ 创建笔记「块引用测试」: ${p2}`);

  console.log('\n📋 接下来：');
  console.log('   1. 启动 SophoNote（./scripts/sophonote.sh start）');
  console.log('   2. 打开笔记本，找到「块引用测试」');
  console.log('   3. 预览态点击 [[技术调研#^core-arch]] 测试跳转');
  console.log('   4. 预览态查看 ![[技术调研#^core-arch]] 测试嵌入卡片');
} catch (e) {
  console.error('❌ 注入失败:', e.message);
  process.exit(1);
}
