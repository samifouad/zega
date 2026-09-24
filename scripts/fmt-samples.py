"""Apply/check canonical ZQL and JSON in Markdown and the explorer's static examples.

All formatting goes through `zega fmt --stdin`; this script only extracts source
from its host document. Interpolated JS templates are formatted at generation.
"""
import argparse
from pathlib import Path
import re
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("zega", type=Path)
parser.add_argument("--check", action="store_true")
parser.add_argument("--extract", type=Path, help="write samples for AST/idempotence tests")
args = parser.parse_args()
binary = str(args.zega.resolve())
changed = []
count = 0

def format_sample(source, label, lang="zql"):
    global count
    count += 1
    # Bytes, not text mode: on Windows text mode would turn every "\n" sent
    # to the formatter into "\r\n", and the check compares bytes.
    data = source.encode("utf-8")
    result = subprocess.run([binary, "fmt", "--stdin", "--lang", lang], input=data, capture_output=True, check=True)
    formatted = result.stdout.decode("utf-8")
    if args.extract:
        args.extract.mkdir(parents=True, exist_ok=True)
        (args.extract / f"sample-{count}.{lang}").write_bytes(data)
    if args.check:
        checked = subprocess.run([binary, "fmt", "--stdin", "--lang", lang, "--check"], input=data, capture_output=True)
        if checked.returncode not in (0, 1):
            raise RuntimeError(f"{label}: {checked.stderr.decode('utf-8', 'replace')}")
        if checked.returncode:
            changed.append(label)
    return formatted

files = sorted(set(Path('.').glob('*.md')) | set(Path('docs').glob('*.md')) | set(Path('browser').glob('*.md')) | set(Path('browser/docs').glob('*.md')) | set(Path('zega').glob('*.md')))
for path in files:
    original = path.read_bytes().decode("utf-8")
    def fence(match):
        if match[2].strip().lower() not in ('', 'zql', 'json'):
            return match[0]
        line = original[:match.start()].count('\n') + 1
        return match[1] + format_sample(match[3], f"{path}:{line}", match[2].strip().lower() or "zql") + match[4]
    updated = re.sub(r'(^```([^\n]*)\n)(.*?)(^```[ \t]*(?=\n|$))', fence, original, flags=re.M | re.S)
    if not args.check and updated != original:
        path.write_bytes(updated.encode("utf-8"))

path = Path('browser/repl.js')
original = path.read_bytes().decode("utf-8")
def template(match):
    content = match[1]
    if '${' in content or not re.match(r'\s*(?:type\s|schema\s|query\s|mutation\s|\{)', content):
        return match[0]
    # These are JS template literals: preserve their existing final newline
    # convention while checking the ZQL they represent with its final newline.
    formatted = format_sample(content.rstrip() + '\n', f'{path}:{original[:match.start()].count(chr(10))+1}')
    return '`' + formatted.rstrip('\n') + '`'
updated = re.sub(r'`([^`\\]*)`', template, original)
if not args.check and updated != original:
    path.write_bytes(updated.encode("utf-8"))
print(f'checked {count} embedded ZQL/JSON samples')
for label in changed:
    print(f'{label} would be reformatted')
raise SystemExit(bool(changed))
