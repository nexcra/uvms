#!/usr/bin/env python3
"""Compatibility entry point for the standalone Java project."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
raise SystemExit(subprocess.call([sys.executable, str(root / 'java/scripts/verify-java-package.py'),
                                 '--dist', str(root / 'dist'), *sys.argv[1:]]))
