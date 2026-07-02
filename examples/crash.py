#!/usr/bin/env python3
"""Demonstrates that a crashing script does NOT take down the manager:
it raises an unhandled exception, so pyman-worker exits non-zero and the
GUI marks the task as Failed while staying alive itself."""
import sys

print("about to crash...", flush=True)
raise ValueError("intentional crash from crash.py")
