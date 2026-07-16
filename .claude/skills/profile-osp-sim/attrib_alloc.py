#!/usr/bin/env python3
"""Attribute allocation samples in a macOS `sample` call graph to their nearest
user-code (osp_core / integer_blossom / osp_sim) caller.

Usage:  python3 attrib_alloc.py <sample_output.txt>

Prints the functions responsible for the most malloc/free/realloc/Vec-grow/
HashMap-rehash samples, so you can see where the allocation churn comes from
rather than just that it exists.
"""
import re
import sys
import collections

lines = open(sys.argv[1]).read().splitlines()
# Isolate the "Call graph:" section (indented caller->callee tree).
start = next(i for i, l in enumerate(lines) if l.startswith('Call graph:'))
try:
    end = next(i for i, l in enumerate(lines) if 'Total number in stack' in l)
except StopIteration:
    end = len(lines)
lines = lines[start + 1:end]

alloc_pat = re.compile(
    r'malloc|free|realloc|finish_grow|grow_one|reserve_rehash|RawVec|'
    r'from_iter|to_vec|memmove|memset|alloc::')


def parse(l):
    # Each line: leading tree chars (" .!:|+"), then the inclusive count, then name.
    m = re.match(r'^([ .!:|+]*)(\d+)\s', l)
    if not m:
        return None, None, None
    return len(m.group(1)), int(m.group(2)), l[m.end():].strip()


stack = []  # (indent_depth, name) of the current ancestry
attrib = collections.Counter()
for l in lines:
    d, cnt, name = parse(l)
    if d is None:
        continue
    while stack and stack[-1][0] >= d:
        stack.pop()
    if alloc_pat.search(name):
        anc = None
        for _, pn in reversed(stack):
            if ('osp_core' in pn or 'integer_blossom' in pn or 'osp_sim' in pn) \
                    and not alloc_pat.search(pn):
                anc = pn
                break
        if anc is None and stack:
            anc = stack[-1][1]
        key = re.sub(r'::h[0-9a-f]+.*', '', anc) if anc else '<root>'
        key = re.sub(r'\s*\(in .*', '', key)
        attrib[key] += cnt
    stack.append((d, name))

for k, v in attrib.most_common(25):
    print(f"{v:7d}  {k}")
