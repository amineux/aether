import hashlib
import importlib.metadata
import json
import os
import pathlib
import platform
import signal
import subprocess
import sys
import tempfile


def write_json(path, data):
    contents = json.dumps(data, indent=2, allow_nan=False) + '\n'
    atomic_text(path, contents)


def atomic_text(path, contents):
    name = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=path.parent,
                                         prefix='.' + path.name, delete=False) as stream:
            name = stream.name
            stream.write(contents)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
    finally:
        if name:
            pathlib.Path(name).unlink(missing_ok=True)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def environment():
    from . import __version__
    versions = {}
    for name in ('iree-base-compiler', 'iree-base-runtime', 'numpy'):
        try:
            versions[name] = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            versions[name] = None
    git = {}
    # Only attribute source to the checkout containing this installed package.
    for root in pathlib.Path(__file__).resolve().parents:
        if (root / '.git').exists():
            for key, args in [('commit', ['rev-parse', 'HEAD']), ('changes', ['status', '--porcelain'])]:
                try:
                    proc = subprocess.run(['git', '-C', str(root), *args], capture_output=True, text=True, timeout=5)
                    git[key] = proc.stdout.strip() if proc.returncode == 0 else None
                except (OSError, subprocess.TimeoutExpired):
                    git[key] = None
            break
    return {'toolkit_version': __version__, 'python': sys.version, 'platform': platform.platform(),
            'machine': platform.machine(), 'cpu_count': os.cpu_count(), 'packages': versions,
            'source': git or {'commit': None, 'note': 'Installed distribution; no source checkout available'}}


def run_process(command, timeout):
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, encoding='utf-8', errors='replace', start_new_session=os.name == 'posix')
    def stop():
        try:
            if os.name == 'posix':
                os.killpg(proc.pid, signal.SIGKILL)
            else:
                proc.kill()
        except ProcessLookupError:
            pass
    try:
        out, err = proc.communicate(timeout=timeout)
        return proc.returncode, out, err
    except subprocess.TimeoutExpired:
        stop()
        out, err = proc.communicate()
        return 124, out, err + '\nEvaluation timed out; worker terminated.\n'
    except BaseException:
        stop()
        proc.communicate()
        raise
