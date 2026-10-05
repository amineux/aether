#!/usr/bin/env python3
"""Render a portable investor evaluation report from a fresh host evidence bundle.

Checksums detect accidental edits, not forged evidence. This is not attestation.
Never treats host checks as customer, hardware, or production validation.
"""
import argparse
import hashlib
import html
import json
import pathlib
import re
import sys

from collect_evidence import COMMANDS


def validate(bundle):
    data = json.loads((bundle / 'manifest.json').read_text(encoding='utf-8'))
    issues = []
    if data.get('schema_version') != 2:
        issues.append('Unsupported evidence schema; collect a fresh bundle.')
    if data.get('complete') is not True or data.get('passed') is not True:
        issues.append('Evidence collection failed or is incomplete.')
    meta = data.get('metadata', {})
    for key in ('commit', 'working_tree', 'working_tree_end', 'rustc', 'cargo'):
        record = meta.get(key, {})
        if record.get('exit_code') != 0:
            issues.append(f'Missing or failed provenance: {key}.')
    sha = meta.get('commit', {}).get('stdout', '').strip()
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        issues.append('Invalid source commit.')
    for key in ('working_tree', 'working_tree_end'):
        if meta.get(key, {}).get('stdout', '').strip():
            issues.append(f'Source tree was modified ({key}); commit and recapture.')
    lock = bundle / 'Cargo.lock'
    if not lock.is_file() or hashlib.sha256(lock.read_bytes()).hexdigest() != data.get('lockfile_sha256'):
        issues.append('Cargo.lock is missing or changed.')
    checks = data.get('checks', [])
    expected = dict(COMMANDS)
    if [c.get('name') for c in checks] != list(expected):
        issues.append('Required checks are missing, duplicated or out of order.')
    for check in checks:
        name = check.get('name', '')
        if name not in expected:
            issues.append('Unknown check in manifest.')
            continue  # Never use an untrusted name as a file path.
        if check.get('command') != expected[name]:
            issues.append(f'{name}: unexpected command.')
        if (check.get('passed') is not True or check.get('exit_code') != 0
                or check.get('missing_expected_lines') != []):
            issues.append(f'{name}: check failed or expected output is missing.')
        for stream in ('stdout', 'stderr'):
            path = bundle / f'{name}.{stream}.log'
            digest = check.get('logs', {}).get(stream)
            if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                issues.append(f'{name}: {stream} log is missing or changed.')
    return data, issues


def render(data, issues):
    esc = lambda value: html.escape(str(value), quote=True)
    meta = data.get('metadata', {})
    sha = meta.get('commit', {}).get('stdout', 'unknown').strip()
    rows = ''.join(f'<tr><td>{esc(c.get("name"))}</td><td>{"PASS" if c.get("passed") is True and c.get("exit_code") == 0 else "FAIL"}</td>'
                   f'<td><code>{esc(" ".join(c.get("command", [])))}</code></td></tr>' for c in data.get('checks', []))
    errors = ''.join(f'<li>{esc(issue)}</li>' for issue in issues)
    logs = ''
    # Logs remain adjacent files; no external assets, scripts, or requests.
    for name, _ in COMMANDS:
        logs += f'<li>{esc(name)}: <a href="{name}.stdout.log">stdout</a> · <a href="{name}.stderr.log">stderr</a></li>'
    status = 'NEEDS ATTENTION' if issues else 'HOST EVIDENCE VERIFIED'
    return f'''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Aether · Investor evaluation</title>
<style>body{{margin:0;background:#0d1420;color:#e6ecf5;font:16px/1.6 system-ui,sans-serif}}main{{max-width:1050px;margin:auto;padding:48px 24px}}h1{{font-size:clamp(36px,6vw,64px);line-height:1.1}}h2{{margin-top:40px}}a{{color:#8edcdb}}.eyebrow{{color:#8edcdb;letter-spacing:.14em}}.status,section{{padding:20px;border:1px solid #3c5068;border-radius:12px;margin:24px 0}}.status{{border-color:{'#f3ba64' if issues else '#8edcdb'}}}table{{width:100%;border-collapse:collapse}}td,th{{padding:12px;text-align:left;border-bottom:1px solid #3c5068}}code{{overflow-wrap:anywhere}}small{{color:#a8b9cf}}@media print{{body{{background:white;color:black}}a{{color:#164e63}}main{{padding:0}}section{{break-inside:avoid}}}}</style>
<main><p class="eyebrow">AETHER / RESEARCH EVALUATION</p>
<h1>Isolation for shared<br>AI accelerators.</h1>
<p>A capability-based isolation prototype for accelerator runtime teams.
The initial buyer hypothesis is small NPU companies that need to contain workload faults and protect tenant memory.</p>
<div class="status"><strong>{status}</strong><p>Captured {esc(data.get('captured_at_utc', 'unknown'))}<br>
Source <code>{esc(sha)}</code></p><ul>{errors}</ul>
<small>Verification checks bundle consistency and clean source provenance. It is not independent attestation or production certification.</small></div>
<h2>What the demo establishes</h2>
<p>Host software models exercise capability boundaries, named refused attacks, a frozen command packet, completion events and scoped memory grants.
This evidence does not establish hardware isolation, throughput superiority or customer adoption.</p>
<div style="overflow-x:auto"><table><thead><tr><th>Check</th><th>Recorded result</th><th>Reproduction command</th></tr></thead><tbody>{rows}</tbody></table></div>
<section><h2>The first commercial offer</h2><p>A bounded evaluation: one customer workload, one environment, an agreed baseline, fault-containment acceptance criteria and an end date.
The proposed progression is paid integration followed by reusable software and support. Demand, pricing and repeatability remain unverified.</p></section>
<h2>Investment milestones</h2><ol><li>Confirm repeated urgent pain with independent accelerator teams and name a budget owner.</li>
<li>Secure runtime or hardware access and have another engineer reproduce an agreed workload against a baseline.</li>
<li>Complete a paid pilot, then demonstrate reuse with a second buyer.</li></ol>
<h2>Open diligence gates</h2><p>Customer validation, revenue, real hardware enforcement, production runtime integration and independent security review remain unverified by this bundle.
Review open security issues and pull requests at the pinned revision before any deployment claim. Test counts are not a measure of product readiness.</p>
<h2>Inspect the evidence</h2><ul>{logs}</ul><p><a href="manifest.json">Manifest</a> · <a href="Cargo.lock">Dependency lockfile</a></p>
<p><small>Share this HTML with its entire evidence directory. Review logs for sensitive content before sharing. Checksum consistency detects accidental changes; it cannot authenticate an author or prove that commands ran.</small></p></main></html>'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=pathlib.Path)
    args = parser.parse_args()
    try:
        data, issues = validate(args.bundle)
        output = args.bundle / 'investor-report.html'
        output.write_text(render(data, issues), encoding='utf-8')
    except (OSError, ValueError, TypeError, AttributeError, KeyError) as exc:
        print(f'Cannot render evidence: {exc}', file=sys.stderr)
        return 2
    print(f'{output}: {"needs attention" if issues else "host evidence verified"}')
    return 1 if issues else 0


if __name__ == '__main__':
    sys.exit(main())
