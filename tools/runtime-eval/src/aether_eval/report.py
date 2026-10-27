"""Offline report; unsupported security checks can never appear as passing."""
import html
from .common import atomic_text


def render(result, output):
    esc = lambda x: html.escape(str(x), quote=True)
    status = 'CHECKS PASSED' if result.get('complete') is True and result.get('passed') is True else 'NEEDS ATTENTION'
    rows = ''
    for name, entry in result.get('summary', {}).items():
        rows += f'<tr><td>{esc(name)}</td><td>{entry["solo"]["p95_ms"]:.3f}</td><td>{entry["shared"]["p95_ms"]:.3f}</td><td>{entry["p95_slowdown"]:.2f}×</td><td>{esc(entry["latency_gate"]["status"])}</td></tr>'
    checks = ''.join(f'<li>{esc(c["workload"])} / {esc(c["check"])}: {"PASS" if c["passed"] else "FAIL"}</li>' for c in result.get('checks', []))
    limits = ''.join(f'<li>{esc(x)}</li>' for x in result.get('limitations', []))
    error = f'<p>{esc(result["error"])}</p>' if result.get('error') else ''
    atomic_text(output / 'report.html', f'''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>Aether runtime evaluation</title>
<style>body{{font:16px/1.6 system-ui;background:#0d1420;color:#e6ecf5;margin:0}}main{{max-width:1000px;margin:auto;padding:40px 24px}}a{{color:#8edcdb}}h1{{font-size:42px}}td,th{{padding:12px;border-bottom:1px solid #3c5068;text-align:left}}.box{{border:1px solid #3c5068;border-radius:12px;padding:20px;margin:24px 0}}table{{width:100%}}@media print{{body{{background:white;color:black}}}}</style>
<main><p>AETHER / EXTERNAL RUNTIME EVALUATION</p><h1>Two workloads. One CPU device.</h1>
<div class="box"><strong>{status}</strong>{error}<p>Real IREE execution. A passed run means correctness and requested latency gates passed; it is not proof of speed superiority or hardware isolation.</p></div>
<h2>Invocation latency</h2><p>p95 in milliseconds, nearest-rank percentile. Shared / solo ratios above 1 indicate slower observed shared execution. Compare repeated rounds and raw samples before drawing conclusions.</p>
<div style="overflow-x:auto"><table><tr><th>Workload</th><th>Solo p95</th><th>Shared p95</th><th>Ratio</th><th>Optional gate</th></tr>{rows}</table></div>
<h2>Runtime checks</h2><ul>{checks}</ul><h2>Scope and limitations</h2><ul>{limits}</ul>
<p><strong>Hardware isolation: unsupported. Aether kernel policy enforcement: unsupported.</strong></p>
<h2>Inspect and replay</h2><p><a href="result.json">Result and environment</a> · <a href="scenario.json">Scenario</a> · <a href="samples.json">Raw samples</a> · <a href="artifacts.json">Artifact hashes</a> · <a href="worker.stderr.log">Diagnostics</a></p>
<p>Replay this trusted bundle with <code>aether-eval replay BUNDLE --output NEW_DIRECTORY</code>. Replay executes saved compiled code. Hashes detect changes, not a trusted author.</p></main></html>''')
