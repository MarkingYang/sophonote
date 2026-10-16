#!/usr/bin/env python3
"""Run the 80 named macOS Host contracts; never promote them to UI/E2E approval.

Reports are disposable build artifacts under src-tauri/target. Product progress
and remaining release gates belong only in docs/project-ledger.md.
"""
import argparse
import collections
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'src-tauri/tests/fixtures/agent-routing-acceptance.json'
NATIVE = [
    'agent::pi::tests::native_pi_roundtrip_approval_history_and_plan_denial',
    'agent::pi::tests::native_pi_anthropic_roundtrip_approval_history_and_plan_denial',
    'agent::claude::tests::native_claude_tools_history_plan_error_and_cancel',
    'agent::opencode::tests::native_opencode_tools_history_plan_error_and_cancel',
    'agent::opencode::tests::native_opencode_anthropic_tools_history_plan_error_and_cancel',
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--group', choices=list('ABCDEFGH'), action='append')
    parser.add_argument('--native', action='store_true', help='also run five real binary/local model fixtures')
    parser.add_argument('--list', action='store_true', help='list specifications without executing')
    args = parser.parse_args()
    data = json.loads(MANIFEST.read_text())
    cases = data['cases']
    if len(cases) != 80 or collections.Counter(c['group'] for c in cases) != dict.fromkeys('ABCDEFGH', 10):
        raise ValueError('catalog must contain eight groups of ten independent cases')
    for field in ('id', 'contractTest'):
        if len({c[field] for c in cases}) != 80:
            raise ValueError(f'duplicate {field}')
    selected = [c for c in cases if not args.group or c['group'] in args.group]
    if args.list:
        for case in selected:
            print(f"{case['id']} {case['title']} -> {case['contractTest']}")
        return 0
    if sys.platform != 'darwin':
        raise RuntimeError('this acceptance runner targets the macOS host; Windows evidence is separate')
    env = os.environ.copy()
    # Local HTTP fixtures must not be intercepted by a system proxy. No persistent change.
    env['NO_PROXY'] = env['no_proxy'] = '127.0.0.1,localhost'
    env['PATH'] = str(Path.home() / '.cargo/bin') + os.pathsep + env.get('PATH', '')
    output = ROOT / 'src-tauri/target/routing-acceptance-results'
    output.mkdir(parents=True, exist_ok=True)
    (output / 'results.json').unlink(missing_ok=True)  # never expose a previous pass during a new run
    print('Building test executable (no production app or real provider is started)', flush=True)
    build = subprocess.run(['cargo', 'test', '--lib', '--no-run', '--message-format=json'],
                           cwd=ROOT / 'src-tauri', env=env, capture_output=True, text=True, timeout=300)
    (output / 'build.log').write_text(build.stderr + build.stdout)
    if build.returncode:
        print('Build failed; see src-tauri/target/routing-acceptance-results/build.log', file=sys.stderr)
        return 1
    binaries = []
    for line in build.stdout.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get('reason') == 'compiler-artifact' and event.get('executable') and event.get('profile', {}).get('test'):
            binaries.append(event['executable'])
    if len(binaries) != 1:
        raise RuntimeError(f'expected one lib test executable, found {len(binaries)}')
    binary = binaries[0]
    listing = subprocess.run([binary, '--list'], cwd=ROOT / 'src-tauri', env=env,
                             capture_output=True, text=True, check=True, timeout=30).stdout
    known = {line.removesuffix(': test') for line in listing.splitlines() if line.endswith(': test')}
    tasks = [(c['id'], c['title'], c['contractTest'], False) for c in selected]
    if args.native:
        tasks.extend((f'N{i}', '真实二进制/本地模型协议合同', name, True) for i, name in enumerate(NATIVE, 1))
    missing = {name for _, _, name, _ in tasks} - known
    if missing:
        raise RuntimeError(f'catalog targets missing from compiled test inventory: {sorted(missing)}')
    results = []
    for ident, title, name, ignored in tasks:
        command = [binary, '--exact', name, '--nocapture']
        if ignored:
            command.append('--ignored')
        start = time.monotonic()
        process = subprocess.Popen(command, cwd=ROOT / 'src-tauri', env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, start_new_session=True)
        try:
            stdout, stderr = process.communicate(timeout=60)
            log = stdout + stderr
            # Exit zero with 0 tests or a skipped test is never evidence of a pass.
            passed = process.returncode == 0 and bool(re.search(r'test result: ok\. 1 passed; 0 failed; 0 ignored;', log))
            status = 'passed' if passed else 'failed'
        except subprocess.TimeoutExpired:
            status, log = 'timed_out', 'Test exceeded the 60-second acceptance deadline.\n'
        finally:
            # The process group contains only this test and its native fixtures.
            # Clean native descendants even if the Rust test panics or times out.
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.killpg(process.pid, sig)
                except ProcessLookupError:
                    break
                if sig == signal.SIGTERM:
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        pass
            process.wait()
        (output / f'{ident}.log').write_text(log)
        results.append(dict(id=ident, title=title, test=name, status=status,
                            testProcessElapsedMs=round((time.monotonic()-start)*1000),
                            evidence='native_local_fixture' if ignored else 'host_contract'))
        print(f'{ident} {status}: {title}', flush=True)
    fingerprint = hashlib.sha256()
    for path in sorted((ROOT / 'src-tauri/src').rglob('*.rs')):
        fingerprint.update(str(path.relative_to(ROOT)).encode())
        fingerprint.update(path.read_bytes())
    fingerprint.update(MANIFEST.read_bytes())
    fingerprint.update(Path(__file__).read_bytes())
    fingerprint.update((ROOT / "src-tauri/Cargo.lock").read_bytes())
    report = dict(schemaVersion=1, generatedAt=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                  sourceSha256=fingerprint.hexdigest(), results=results,
                  hostAndRealProviderAcceptance='not_evaluated', releaseReady=False,
                  note='测试进程耗时不是产品路由延迟；合同通过不等于真实供应商/宿主/发布门禁通过。')
    (output / 'results.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    passed = sum(r['status'] == 'passed' for r in results)
    print(f'{passed}/{len(results)} contracts passed. Host/E2E and release readiness are NOT asserted.')
    return int(passed != len(results))


if __name__ == '__main__':
    raise SystemExit(main())
