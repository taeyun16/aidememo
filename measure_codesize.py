#!/usr/bin/env python3
"""Measure codebase size by area, excluding lockfiles and generated content."""

import os
import subprocess
from pathlib import Path

def count_lines(path_pattern):
    """Count lines using tokei, excluding lockfiles and generated files."""
    try:
        result = subprocess.run(
            ['tokei', path_pattern, '--exclude', '*.lock', '--exclude', 'node_modules', 
             '--exclude', 'target', '--exclude', '.git', '--exclude', 'build'],
            capture_output=True,
            text=True,
            cwd='/workspace'
        )
        # Parse tokei output for total lines
        lines = result.stdout.strip().split('\n')
        for line in lines:
            if 'Total' in line or 'Lines' in line:
                parts = line.split()
                for i, part in enumerate(parts):
                    if part.isdigit() and int(part) > 100:
                        return int(part)
        return 0
    except Exception as e:
        return 0

def count_files_and_lines(base_path, extensions, exclude_patterns=None):
    """Count files and lines for given extensions."""
    exclude_patterns = exclude_patterns or []
    files = []
    total_lines = 0
    
    for ext in extensions:
        for root, dirs, filenames in os.walk(base_path):
            # Skip excluded directories
            if any(excl in root for excl in ['node_modules', 'target', '.git', 'build']):
                continue
            
            for filename in filenames:
                if filename.endswith(ext):
                    # Skip lockfiles
                    if 'lock' in filename.lower():
                        continue
                    
                    full_path = os.path.join(root, filename)
                    
                    # Skip excluded patterns
                    if any(excl in full_path for excl in exclude_patterns):
                        continue
                    
                    try:
                        with open(full_path, 'r', encoding='utf-8', errors='ignore') as f:
                            lines = len(f.readlines())
                            total_lines += lines
                            files.append((full_path, lines))
                    except:
                        pass
    
    return len(files), total_lines, files

# Measure different areas
areas = {
    'aidememo-core': ('crates/aidememo-core', ['.rs']),
    'aidememo-cli': ('crates/aidememo-cli', ['.rs']),
    'aidememo-server': ('crates/aidememo-server', ['.rs']),
    'aidememo-domain': ('crates/aidememo-domain', ['.rs']),
    'aidememo-service': ('crates/aidememo-service', ['.rs']),
    'aidememo-client': ('crates/aidememo-client', ['.rs']),
    'aidememo-artifacts': ('crates/aidememo-artifacts', ['.rs']),
    'store-local': ('crates/aidememo-store-local', ['.rs']),
    'other-crates': ('crates', ['.rs']),  # Will subtract the above
    'docs': ('docs', ['.md', '.mdx']),
    'website-src': ('website', ['.js', '.jsx', '.css', '.md', '.mdx']),
    'tests': ('tests', ['.rs']),
    'examples': ('examples', ['.rs', '.md']),
    'scripts': ('scripts', ['.py', '.sh', '.bash']),
    'root-docs': ('.', ['.md']),
}

print("CODEBASE SIZE MEASUREMENT (excluding lockfiles, node_modules, target)")
print("=" * 80)

results = {}
for area, (path, extensions) in areas.items():
    full_path = os.path.join('/workspace', path)
    if os.path.exists(full_path):
        file_count, line_count, _ = count_files_and_lines(full_path, extensions)
        results[area] = (file_count, line_count)
        print(f"{area:20} {file_count:5} files  {line_count:8} LOC")

# Calculate other-crates properly (subtract known crates)
if 'other-crates' in results:
    known_crates_lines = sum([
        results.get('aidememo-core', (0, 0))[1],
        results.get('aidememo-cli', (0, 0))[1],
        results.get('aidememo-server', (0, 0))[1],
        results.get('aidememo-domain', (0, 0))[1],
        results.get('aidememo-service', (0, 0))[1],
        results.get('aidememo-client', (0, 0))[1],
        results.get('aidememo-artifacts', (0, 0))[1],
        results.get('store-local', (0, 0))[1],
    ])
    other_lines = results['other-crates'][1] - known_crates_lines
    if other_lines > 0:
        print(f"{'  (other crates)':20} {'?':5} files  {other_lines:8} LOC")

print("=" * 80)
total_files = sum(r[0] for r in results.values())
total_lines = sum(r[1] for r in results.values())
print(f"{'TOTAL':20} {total_files:5} files  {total_lines:8} LOC")
