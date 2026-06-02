import os
from pathlib import Path

PACKAGE_NAME = "openai-codex-cli-bin"
PACKAGE_METADATA_FILENAME = "codex-package.json"


def bundled_package_dir() -> Path:
    path = Path(__file__).resolve().parent
    metadata_path = path / PACKAGE_METADATA_FILENAME
    if not metadata_path.is_file():
        raise FileNotFoundError(
            f"{PACKAGE_NAME} is installed but missing its package metadata at {metadata_path}"
        )
    return path


def bundled_codex_path() -> Path:
    exe = "codewith.exe" if os.name == "nt" else "codewith"
    path = bundled_package_dir() / "bin" / exe
    if not path.is_file():
        legacy_exe = "codex.exe" if os.name == "nt" else "codex"
        legacy_path = bundled_package_dir() / "bin" / legacy_exe
        if legacy_path.is_file():
            return legacy_path
        raise FileNotFoundError(
            f"{PACKAGE_NAME} is installed but missing its packaged Codewith binary at {path}"
        )
    return path


def bundled_codewith_path() -> Path:
    return bundled_codex_path()


def bundled_path_dir() -> Path | None:
    for name in ("codewith-path", "codex-path"):
        path = bundled_package_dir() / name
        if path.is_dir():
            return path
    return None


__all__ = [
    "PACKAGE_NAME",
    "bundled_codewith_path",
    "bundled_codex_path",
    "bundled_package_dir",
    "bundled_path_dir",
]
