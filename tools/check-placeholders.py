#!/usr/bin/env python3
"""check-placeholders.py: every translatable text with {places} in it gets
them filled, and only those (see src/i18n.rs).

A tr("…{a}…") or trn("…", "…", n) right inside fill(…, &[("a", …)]) (or
props.rs's subst) is checked against the names given there; a text with
places that isn't filled where it's written (kept in a variable, passed
on) is listed for a look by hand, unless a "check-placeholders: filled
later" comment is on its line or up to four lines above (over a match). Also each .po file's
translations keep the same places as their originals (msgfmt --check does
this too). Exit status 1 if anything is wrong."""
import re
import subprocess
import sys

PLACE = re.compile(r"(?<![{$])\{([a-z_]+)\}(?!\})")
STR = r'"((?:[^"\\]|\\.)*)"'
problems, later = [], []


def matching_paren(s, i):
    """The index of the ")" closing the "(" at s[i], skipping strings."""
    depth, j = 0, i
    while j < len(s):
        c = s[j]
        if c == '"':
            j += 1
            while s[j] != '"':
                j += 2 if s[j] == "\\" else 1
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return j
        j += 1
    return -1


files = subprocess.run(["git", "ls-files", "src/*.rs", "src/**/*.rs"], capture_output=True, text=True).stdout.split()
for path in sorted(set(files)):
    src = open(path).read()
    cut = re.search(r"\n#\[cfg\(test\)\]\nmod tests", src)
    body = src[: cut.start()] if cut else src
    for m in re.finditer(r"\b(tr|trn|n_)\(\s*" + STR + r"(?:\s*,\s*" + STR + r")?", body):
        texts = [t for t in (m.group(2), m.group(3)) if t is not None]
        places = set().union(*(set(PLACE.findall(t)) for t in texts))
        line = body.count("\n", 0, m.start()) + 1
        where = f"{path}:{line}"
        if m.group(3) is not None and set(PLACE.findall(m.group(2))) - {"n"} != set(PLACE.findall(m.group(3))) - {"n"}:
            problems.append(f"{where}: singular and plural have different places: {texts}")
        if not places:
            continue
        before = body[max(0, m.start() - 40) : m.start()]
        f = re.search(r"\b(fill|subst)\(\s*&$", before)
        if not f:
            lines = body.split("\n")
            # The comment may stand above a match whose arms these are.
            near = " ".join(lines[max(0, line - 5) : line])
            if "check-placeholders: filled later" not in near:
                later.append(f"{where}: {m.group(1)}(\"{texts[0][:70]}\") with {sorted(places)}")
            continue
        start = m.start() - len(before) + f.start()
        end = matching_paren(body, body.index("(", start))
        span = body[start:end]
        # Names given to fills nested inside this one aren't this one's.
        inner = [(k.start(), matching_paren(span, span.index("(", k.start()))) for k in re.finditer(r"\b(fill|subst)\(", span) if k.start() > 0]
        for a, z in reversed(inner):
            span = span[:a] + " " * (z - a + 1) + span[z + 1 :]
        given = set(re.findall(r'\(\s*"([a-z_]+)"\s*,', span))
        if places - given:
            problems.append(f"{where}: not filled: {sorted(places - given)} in \"{texts[0][:70]}\"")
        if given - places:
            problems.append(f"{where}: filled but not in the text: {sorted(given - places)} in \"{texts[0][:70]}\"")

# The translations: the same places as the originals.
for po in subprocess.run(["git", "ls-files", "po/*.po"], capture_output=True, text=True).stdout.split():
    text = open(po).read()
    for block in text.split("\n\n"):
        def val(key):
            m = re.search(r"^" + re.escape(key) + r" ((?:\"(?:[^\"\\]|\\.)*\"\s*)+)", block, re.M)
            return "".join(re.findall(STR, m.group(1))) if m else None
        msgid = val("msgid")
        if not msgid:
            continue
        want = set(PLACE.findall(msgid)) | set(PLACE.findall(val("msgid_plural") or ""))
        for key in ("msgstr", "msgstr[0]", "msgstr[1]", "msgstr[2]"):
            got = val(key)
            if got and set(PLACE.findall(got)) != want and not (key == "msgstr[0]" and set(PLACE.findall(got)) == want - {"n"}):
                problems.append(f"{po}: {key} of \"{msgid[:60]}\" has {sorted(set(PLACE.findall(got)))}, the original {sorted(want)}")

for p in problems:
    print("PROBLEM", p)
if later:
    print(f"{len(later)} texts with places filled elsewhere (check by hand):")
    for p in later:
        print("  ", p)
print(f"{len(problems)} problems")
sys.exit(1 if problems else 0)
