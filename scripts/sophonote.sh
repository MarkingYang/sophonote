#!/bin/bash
# SophoNote 开发模式生命周期管理
# 用法: ./scripts/sophonote.sh {start|stop|restart|status|logs|skills}
#
# Hermes Client Surface：在项目根创建 .env.hermes.local（已 gitignore *.local）：
#   SOPHONOTE_HERMES_ATTACH_EXTERNAL=1
#   SOPHONOTE_HERMES_GATEWAY_URL=ws://127.0.0.1:9119/api/ws
#   SOPHONOTE_HERMES_GATEWAY_TOKEN=… # 与 HERMES_DASHBOARD_SESSION_TOKEN 一致，勿入库
#   SOPHONOTE_HERMES_HOME=/absolute/path/.hermes
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_FILE="$ROOT/.dev.pid"
LOG_DIR="$ROOT/logs"
LOG_FILE="$LOG_DIR/dev.log"
HERMES_ENV_FILE="$ROOT/.env.hermes.local"
HERMES_PID_FILE="$ROOT/.hermes-surface.pid"
HERMES_TOKEN_FILE="$ROOT/.hermes-surface.token"
HERMES_LOG_FILE="$LOG_DIR/hermes-surface.log"
HERMES_MANAGED_PORT=9119

export PATH="$HOME/.cargo/bin:$PATH"

mkdir -p "$LOG_DIR"

# 加载开发附着环境（密钥不入库；仅本机）
load_hermes_attach_env() {
  if [ -f "$HERMES_ENV_FILE" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$HERMES_ENV_FILE"
    set +a
  fi
}

managed_hermes_running() {
  [ -f "$HERMES_PID_FILE" ] && kill -0 "$(cat "$HERMES_PID_FILE")" 2>/dev/null
}

external_hermes_enabled() {
  case "${SOPHONOTE_HERMES_ATTACH_EXTERNAL:-}" in
    1|true|TRUE|yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

sync_hermes_skills() {
  if ! external_hermes_enabled; then
    echo "ℹ️  默认包内 Hermes 的 Skills 来自 resources；修改 Skill 后请执行 pnpm hermes:bundle"
    return 0
  fi
  if [ -z "${SOPHONOTE_HERMES_HOME:-}" ]; then
    echo "❌ 外部附着必须设置绝对路径 SOPHONOTE_HERMES_HOME"
    return 1
  fi
  SOPHONOTE_HERMES_HOME="$SOPHONOTE_HERMES_HOME" "$ROOT/scripts/sync-hermes-skills.sh"
}

find_hermes_bin() {
  if [ -n "${SOPHONOTE_HERMES_BIN:-}" ] && [ -x "${SOPHONOTE_HERMES_BIN}" ]; then
    echo "${SOPHONOTE_HERMES_BIN}"
  elif [ -x "$HOME/.hermes/hermes-agent/venv/bin/hermes" ]; then
    echo "$HOME/.hermes/hermes-agent/venv/bin/hermes"
  else
    command -v hermes 2>/dev/null || true
  fi
}

# 未显式附着 Gateway 时，像 Hermes Desktop 一样拉起一个由当前 Surface 拥有的
# loopback Runtime。Token 只落在 gitignore 的本机文件，既不写日志也不进仓库。
ensure_hermes_surface() {
  if ! external_hermes_enabled; then
    # 旧 .env 文件不能让 Debug 静默回退到机器 ~/.hermes；清掉遗留变量，
    # 由 Tauri Host 启动 resources 中的钉扎 Sidecar。
    unset SOPHONOTE_HERMES_GATEWAY_URL SOPHONOTE_HERMES_GATEWAY_TOKEN SOPHONOTE_HERMES_HOME
    return 0
  fi
  if [ -z "${SOPHONOTE_HERMES_GATEWAY_URL:-}" ] || \
     [ -z "${SOPHONOTE_HERMES_GATEWAY_TOKEN:-}" ] || \
     [ -z "${SOPHONOTE_HERMES_HOME:-}" ]; then
    echo "❌ 外部附着需同时配置 ATTACH_EXTERNAL、GATEWAY_URL、GATEWAY_TOKEN 与 HERMES_HOME。"
    return 1
  fi
  return 0
}

stop_managed_hermes() {
  if ! managed_hermes_running; then
    rm -f "$HERMES_PID_FILE"
    return 0
  fi
  local pid
  pid=$(cat "$HERMES_PID_FILE")
  echo "⚕️  停止 SophoNote 托管的 Hermes Runtime (PID $pid)..."
  kill "$pid" 2>/dev/null || true
  sleep 1
  kill -9 "$pid" 2>/dev/null || true
  rm -f "$HERMES_PID_FILE"
}

hermes_attach_status() {
  if external_hermes_enabled; then
    echo "   Hermes Surface: 已配置 → ${SOPHONOTE_HERMES_GATEWAY_URL}"
  else
    echo "   Hermes Surface: 包内钉扎 Sidecar（SophoNote 私有数据目录）"
  fi
}

# 递归杀掉整个进程树（npm → cargo → vite → sophonote 二进制）
kill_tree() {
  local pid="$1"
  local children
  children=$(pgrep -P "$pid" 2>/dev/null || true)
  for child in $children; do
    kill_tree "$child"
  done
  kill "$pid" 2>/dev/null || true
}

is_running() {
  [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null
}

cmd_start() {
  if is_running; then
    echo "⚠️  SophoNote 已在运行 (PID $(cat "$PID_FILE"))，如需重启请用: $0 restart"
    exit 1
  fi
  load_hermes_attach_env
  if external_hermes_enabled; then
    sync_hermes_skills || exit 1
  fi
  ensure_hermes_surface || exit 1
  # 清理残留：绕过脚本直跑二进制的实例（含 ^Z 挂起态——挂起进程收不到 TERM，
  # 必须 -9；否则两份进程抢同一份 SQLite/notes 目录）。托管实例已在上方 is_running 拦截，此处不误伤。
  local stale_bin
  stale_bin=$(pgrep -f "$ROOT/src-tauri/target/debug/sophonote" 2>/dev/null || true)
  if [ -n "$stale_bin" ]; then
    echo "🧹 发现直跑二进制的残留实例 ($stale_bin)，自动清理..."
    pkill -9 -f "$ROOT/src-tauri/target/debug/sophonote" 2>/dev/null || true
    sleep 1
  fi
  # 同一仓库打出的 Debug .app 也会使用相同 bundle id / Application Support。
  # 避免它与 tauri dev 并发争抢 DB、notes 和私有 Sidecar。
  local stale_bundle_pattern="src-tauri/target/debug/bundle.noindex/macos/SophoNote.app/Contents/MacOS/sophonote"
  local stale_bundle
  stale_bundle=$(pgrep -f "$stale_bundle_pattern" 2>/dev/null || true)
  if [ -n "$stale_bundle" ]; then
    echo "🧹 发现同仓库 Debug .app 残留实例 ($stale_bundle)，自动清理..."
    pkill -9 -f "$stale_bundle_pattern" 2>/dev/null || true
    sleep 1
  fi
  # 清理残留：PID 文件丢失但 Vite 端口仍被旧进程占用的情况
  local stale_pids
  stale_pids=$(lsof -ti :1420 2>/dev/null || true)
  if [ -n "$stale_pids" ]; then
    echo "🧹 发现占用 1420 端口的残留进程 ($stale_pids)，自动清理..."
    echo "$stale_pids" | xargs kill -9 2>/dev/null || true
    sleep 1
  fi
  echo "🚀 启动 SophoNote 开发模式..."
  echo "   日志: $LOG_FILE"
  hermes_attach_status
  cd "$ROOT" || exit 1
  nohup pnpm tauri dev >"$LOG_FILE" 2>&1 &
  echo $! >"$PID_FILE"
  echo "   PID: $(cat "$PID_FILE")（首次启动需编译 Rust，约 1-2 分钟）"

  # 等待应用窗口启动（每秒轮询，最多 120 秒；通常 5-40 秒）
  local waited=0
  while [ $waited -lt 120 ]; do
    if grep -q "target/debug/sophonote" "$LOG_FILE" 2>/dev/null; then
      # cargo 打出 Running 行后，应用仍可能因 singleton/初始化失败立即退出；
      # 稳定存活两个采样周期才宣告成功。
      sleep 2
      if is_running; then
        echo "✅ 应用已启动（耗时 ${waited}s），桌面窗口应已打开"
        return 0
      fi
    fi
    if ! is_running; then
      echo "❌ 进程提前退出，最后 20 行日志："
      tail -20 "$LOG_FILE"
      rm -f "$PID_FILE"
      exit 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "⏳ 仍在编译中（已等 ${waited}s），可用 '$0 logs' 查看进度"
}

cmd_stop() {
  if ! is_running; then
    echo "ℹ️  SophoNote 未运行"
    rm -f "$PID_FILE"
    stop_managed_hermes
    return 0
  fi
  local pid
  pid=$(cat "$PID_FILE")
  echo "🛑 停止 SophoNote (PID $pid) 及所有子进程..."
  kill_tree "$pid"
  sleep 1
  # 兜底：清理可能残留的同名二进制（-9 保证挂起态实例也能被收掉）
  pkill -9 -f "$ROOT/src-tauri/target/debug/sophonote" 2>/dev/null || true
  rm -f "$PID_FILE"
  stop_managed_hermes
  echo "✅ 已停止"
}

cmd_status() {
  load_hermes_attach_env
  if is_running; then
    local pid
    pid=$(cat "$PID_FILE")
    echo "🟢 SophoNote 运行中 (PID $pid)"
    hermes_attach_status
    echo "   进程树:"
    pgrep -P "$pid" | while read -r child; do
      ps -p "$child" -o pid=,comm= | sed 's/^/   /'
    done
    echo "   最近日志:"
    tail -3 "$LOG_FILE" 2>/dev/null | sed 's/^/   /'
  else
    echo "⚪ SophoNote 未运行"
    if managed_hermes_running; then
      echo "   Hermes Surface: SophoNote 托管中 → ws://127.0.0.1:${HERMES_MANAGED_PORT}/api/ws"
    else
      hermes_attach_status
    fi
  fi
}

cmd_logs() {
  echo "📜 实时日志 (Ctrl+C 退出): $LOG_FILE"
  tail -f "$LOG_FILE"
}

case "${1:-}" in
  start)   cmd_start ;;
  stop)    cmd_stop ;;
  restart) cmd_stop; cmd_start ;;
  status)  cmd_status ;;
  logs)    cmd_logs ;;
  skills)  load_hermes_attach_env; sync_hermes_skills ;;
  *)
    echo "SophoNote 开发模式管理"
    echo "用法: $0 {start|stop|restart|status|logs|skills}"
    echo ""
    echo "Hermes 附着：复制 .env.hermes.example → .env.hermes.local 并填 KEY，再 restart"
    exit 1
    ;;
esac
