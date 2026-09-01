#!/usr/bin/env bash
# ============================================================
# 走查样例笔记库 · 一键注入 / 清理（B4-B9 人工验收用）
#
# 原理：SophoNote 的 .md 文件是正文真相源，但元数据索引在 SQLite
#       （启动迁移只有 DB→文件单向，直接放文件不会被识别），
#       所以这里同时写 notes/<id>.md + articles 表行（content=''）。
#
# 用法：
#   1. 先退出 SophoNote 应用（或 ./sophonote.sh stop）——避免 SQLite 写冲突
#   2. bash scripts/walkthrough-samples/inject.sh          # 注入 6 篇样例
#   3. 启动应用，笔记本里出现 6 篇「样例」开头的笔记，按 README 走查
#   4. 走查完清理：bash scripts/walkthrough-samples/inject.sh --remove
# ============================================================
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
DATA_DIR="$HOME/Library/Application Support/com.fei.sophonote"
DB="$DATA_DIR/sophonote.db"
NOTES="$DATA_DIR/notes"

# id|title 清单（标题必须与笔记内 [[双链]] 写法逐字一致）
SAMPLES='sample-outline-long|样例·深度学习研读
sample-tasks|样例·任务清单
sample-links-from|样例·双链引用页
sample-links-target|样例·被引用页
sample-template-meeting|样例模板·会议纪要
sample-search-extra|样例·搜索陪练'

command -v sqlite3 >/dev/null 2>&1 || { echo "❌ 未找到 sqlite3 命令（macOS 一般自带）"; exit 1; }

if [ "${1:-}" = "--remove" ]; then
  [ -f "$DB" ] || { echo "❌ 未找到 $DB"; exit 1; }
  sqlite3 "$DB" "DELETE FROM articles WHERE id LIKE 'sample-%';"
  rm -f "$NOTES"/sample-*.md
  echo "✅ 已清理样例笔记（重启应用后列表不再显示）"
  exit 0
fi

# 应用运行时写库有锁冲突风险，强制要求先退出
if pgrep -x sophonote >/dev/null 2>&1 || pgrep -f "target/(debug|release)/sophonote" >/dev/null 2>&1; then
  echo "❌ SophoNote 正在运行：请先退出应用（或 ./sophonote.sh stop）再执行本脚本"
  exit 1
fi
[ -f "$DB" ] || { echo "❌ 未找到数据库 $DB（请先至少启动过一次应用）"; exit 1; }
mkdir -p "$NOTES"

count=0
while IFS='|' read -r id title; do
  [ -f "$DIR/$id.md" ] || { echo "❌ 缺少样例文件 $id.md"; exit 1; }
  cp -f "$DIR/$id.md" "$NOTES/$id.md"
  sqlite3 "$DB" "INSERT OR REPLACE INTO articles (id, item_id, title, content, article_type, edited, created_at) VALUES ('$id', NULL, '$title', '', 'manual', 0, datetime('now'));"
  count=$((count + 1))
done <<EOF
$SAMPLES
EOF

echo "✅ 已注入 $count 篇样例笔记（均以「样例」开头）"
echo "   启动应用即可在笔记本看到；走查步骤见同目录 README.md"
echo "   清理：bash $0 --remove"
