#!/usr/bin/env python3
"""A long-running script to test the Stop button. Pass a count as argv[1]."""
import sys
import time

count = int(sys.argv[1]) if len(sys.argv) > 1 else 1000
for i in range(count):
    print(f"line {i}", flush=True)
    time.sleep(0.5)
