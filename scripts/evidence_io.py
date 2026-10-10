"""Atomic publication for evidence manifests and reports (same filesystem)."""
import os
import pathlib
import tempfile


def atomic_write_text(path, contents):
    path = pathlib.Path(path)
    name = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8',
                                         dir=path.parent, prefix=f'.{path.name}.',
                                         delete=False) as stream:
            name = stream.name
            stream.write(contents)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
    finally:
        if name is not None:
            pathlib.Path(name).unlink(missing_ok=True)
