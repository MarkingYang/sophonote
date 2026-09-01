#!/bin/bash
# Install SophoNote-owned Hermes skills into the active Hermes Home.
# Source remains in this repository; Hermes Runtime remains the runtime truth source.
set -eu

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HERMES_ROOT="${SOPHONOTE_HERMES_HOME:-${HERMES_HOME:-$HOME/.hermes}}"
TARGET_ROOT="$HERMES_ROOT/skills/productivity"
BACKUP_ROOT="$HERMES_ROOT/sophonote-skill-backups"
LEGACY="$TARGET_ROOT/sophonote"
SKILLS="sophonote-ai-radar sophonote-help sophonote-markdown-writing sophonote-note-persistence sophonote-openrouter-rankings"

for skill in $SKILLS; do
  source_dir="$ROOT/skills/hermes/productivity/$skill"
  if [ ! -f "$source_dir/SKILL.md" ]; then
    echo "❌ SophoNote Hermes Skill 源文件不存在: $source_dir/SKILL.md"
    exit 1
  fi
done

mkdir -p "$TARGET_ROOT" "$BACKUP_ROOT"

# The previous SophoNote skill contained a fixed bridge URL/token-reading recipe.
# Archive only that known legacy shape; never move an unrelated user skill.
if [ -f "$LEGACY/SKILL.md" ] \
  && grep -q '127\.0\.0\.1:56946' "$LEGACY/SKILL.md" \
  && grep -q 'sophonote-bridge' "$LEGACY/SKILL.md" \
  && grep -q 'Bearer token' "$LEGACY/SKILL.md"; then
  stamp=$(date +%Y%m%d-%H%M%S)
  mv "$LEGACY" "$BACKUP_ROOT/sophonote-legacy-$stamp"
  echo "ℹ️  已归档旧 SophoNote Bridge Skill（可在 $BACKUP_ROOT 恢复）"
fi

# sophonote-discovery-subscriptions 已融合进 sophonote-ai-radar（v2.0.0）：
# 归档运行时残留，避免旧 Skill 继续被 Hermes 枚举或触发。
if [ -d "$TARGET_ROOT/sophonote-discovery-subscriptions" ]; then
  stamp=$(date +%Y%m%d-%H%M%S)
  mv "$TARGET_ROOT/sophonote-discovery-subscriptions" "$BACKUP_ROOT/sophonote-discovery-subscriptions-$stamp"
  echo "ℹ️  已归档被融合替代的 sophonote-discovery-subscriptions（可在 $BACKUP_ROOT 恢复）"
fi

for skill in $SKILLS; do
  source_dir="$ROOT/skills/hermes/productivity/$skill"
  target="$TARGET_ROOT/$skill"
  if [ -d "$target" ] && diff -qr "$source_dir" "$target" >/dev/null 2>&1; then
    echo "✅ Hermes Skill 已是最新: $skill"
    continue
  fi

  stage="$TARGET_ROOT/.$skill.stage.$$"
  rm -rf "$stage"
  cp -R "$source_dir" "$stage"
  # 数据源 Prompt 是 SophoNote 从数据库生成的运行时引用，不属于仓库种子。
  # 开发期同步主 Skill 时保留它们，避免下一次 Agent Run 暂时退回通用规则。
  if [ "$skill" = "sophonote-ai-radar" ] \
    && [ -d "$target/references/source-policies" ]; then
    mkdir -p "$stage/references"
    cp -R "$target/references/source-policies" "$stage/references/source-policies"
  fi
  if [ -d "$target" ]; then
    stamp=$(date +%Y%m%d-%H%M%S)
    mv "$target" "$BACKUP_ROOT/$skill-$stamp"
  fi
  mv "$stage" "$target"
  echo "✅ 已安装 Hermes Skill: $target"
done
