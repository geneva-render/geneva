#!/usr/bin/env python3
"""Runs a command and prints its wall time, CPU time (user + system) and
peak resident memory, as `wall cpu peak_kb`. GNU time does the same, but
is not on every machine."""
import resource
import subprocess
import sys
import time

t0 = time.monotonic()
code = subprocess.call(sys.argv[1:], stdout=subprocess.DEVNULL)
wall = time.monotonic() - t0
r = resource.getrusage(resource.RUSAGE_CHILDREN)
print(f"{wall:.2f} {r.ru_utime + r.ru_stime:.2f} {r.ru_maxrss}")
sys.exit(code)
