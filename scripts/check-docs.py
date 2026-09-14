#!/usr/bin/env python3
"""Documentation consistency checks (run via scripts/check-docs.sh).

Verifies, without a Rust toolchain:
  1. every relative Markdown link resolves;
  2. every scripts/*.sh|py referenced by current docs exists;
  3. every `zalkanes ...` example in current docs names a real subcommand,
     real flags for that subcommand, and no more positionals than it takes
     (the command tree is parsed from crates/zalkanes-cli/src/main.rs);
  4. every command in docs/developers/quickstart.md is run verbatim by
     scripts/dev-quickstart-test.sh;
  5. the protocol manifest hash stated in current docs and release metadata
     equals SHA-256(protocol/v0.toml);
  6. the testnet activation height agrees between the manifest, the Rust
     constant, and docs/developers/testnet.md;
  7. mainnet activation is None in the manifest and the code, and the docs
     say mainnet is disabled;
  8. the candidate identity in docs/developers/testnet.md matches
     audit/gate-attestations.json and (if the tag is present) git;
  9. the testnet status line reflects audit/gate-attestations.json;
 10. the generated limits table in docs/developers/protocol-limits.md matches
     protocol/v0.toml, the Rust constants match the manifest, and every limit
     is explained; `--fix` regenerates the table;
 11. the runtime's host import list is documented.

Historical audit evidence (audit/CANDIDATE.txt, TESTNET-*.md, ...) is not
checked against current values and must not be rewritten.
"""
import hashlib
import json
import pathlib
import re
import shlex
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SKIP_DIR_NAMES = {"target", "node_modules", ".regtest", ".git", "corpus", "artifacts"}
FIX = "--fix" in sys.argv
NO_GIT = "--no-git" in sys.argv

CURRENT_DOC_GLOBS = ["README.md", "CONTRIBUTING.md", "AGENTS.md", "docs/developers/*.md",
                     "contracts/*/README.md", "docs/operator-guide.md"]
MANIFEST_HASH_SITES = ["docs/developers/testnet.md", "audit/README.md", "audit/AUDITOR-HANDOFF.md"]

errors = []


def fail(msg):
    errors.append(msg)


def rel(p):
    return str(pathlib.Path(p).resolve().relative_to(ROOT))


def all_md():
    for p in ROOT.rglob("*.md"):
        if any(part in SKIP_DIR_NAMES for part in p.relative_to(ROOT).parts):
            continue
        yield p


def current_docs():
    seen = set()
    for g in CURRENT_DOC_GLOBS:
        for p in ROOT.glob(g):
            if p.is_file() and p not in seen:
                seen.add(p)
                yield p


FENCE_RE = re.compile(r"^```([^\n]*)\n(.*?)^```", re.S | re.M)


def fenced_blocks(text):
    for m in FENCE_RE.finditer(text):
        yield m.group(1).strip().lower(), m.group(2)


def strip_code(text):
    text = FENCE_RE.sub("", text)
    return re.sub(r"`[^`\n]*`", "", text)


def joined_lines(body):
    """Join backslash continuations, drop comments/blank lines."""
    out, cur = [], ""
    for raw in body.splitlines():
        line = raw.rstrip()
        if cur:
            line = cur + " " + line.strip()
            cur = ""
        if line.endswith("\\"):
            cur = line[:-1].rstrip()
            continue
        s = line.strip()
        if s and not s.startswith("#"):
            out.append(re.sub(r"\s+", " ", s))
    if cur:
        out.append(re.sub(r"\s+", " ", cur.strip()))
    return out


# ── 1. links ────────────────────────────────────────────────────────────────
LINK_RE = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
for md in all_md():
    for target in LINK_RE.findall(strip_code(md.read_text(encoding="utf-8"))):
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        path = target.split("#", 1)[0]
        if not path:
            continue
        if not (md.parent / path).exists():
            fail(f"{rel(md)}: broken link -> {target}")

# ── 2. script references ────────────────────────────────────────────────────
SCRIPT_RE = re.compile(r"(?<![\w/])(?:\./)?scripts/[A-Za-z0-9_./-]+\.(?:sh|py)")
for md in current_docs():
    for ref in set(SCRIPT_RE.findall(md.read_text(encoding="utf-8"))):
        if not (ROOT / ref.lstrip("./")).is_file():
            fail(f"{rel(md)}: referenced script does not exist: {ref}")

# ── 3. CLI command tree from clap source ────────────────────────────────────
def kebab(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "-", name).lower()


def parse_cli(src):
    """Return {enum_name: {variant_kebab: {"flags": {flag: takes_value}, "positionals": n, "sub": enum}}}."""
    enums = {}
    for m in re.finditer(r"enum (\w+) \{", src):
        name = m.group(1)
        depth, i = 1, m.end()
        while depth and i < len(src):
            depth += {"{": 1, "}": -1}.get(src[i], 0)
            i += 1
        body = src[m.end():i - 1]
        variants, attrs, cur = {}, [], None
        vdepth = 0
        for line in body.splitlines():
            s = line.strip()
            if not s or s.startswith("//"):
                continue
            if s.startswith("#["):
                attrs.append(s)
                continue
            vm = re.match(r"^([A-Z]\w*)\s*(\{|,|$)", s)
            if vdepth == 0 and vm:
                cur = kebab(vm.group(1))
                variants[cur] = {"flags": {}, "positionals": 0, "sub": None}
                attrs = []
                if vm.group(2) == "{":
                    vdepth = 1
                continue
            if vdepth:
                if s.startswith("}"):
                    vdepth = 0
                    continue
                fm = re.match(r"^(\w+):\s*(.+?),?$", s)
                if fm:
                    fname, ftype = fm.group(1), fm.group(2)
                    a = " ".join(attrs)
                    attrs = []
                    if "subcommand" in a:
                        variants[cur]["sub"] = ftype.strip()
                    elif "#[arg(" in a and re.search(r"\blong\b", a):
                        lm = re.search(r'long\s*=\s*"([^"]+)"', a)
                        flag = "--" + (lm.group(1) if lm else kebab(fname).replace("_", "-"))
                        variants[cur]["flags"][flag] = ftype.strip() != "bool"
                    else:
                        variants[cur]["positionals"] += 1
        enums[name] = variants
    return enums


CLI_SRC = (ROOT / "crates/zalkanes-cli/src/main.rs").read_text(encoding="utf-8")
CLI = parse_cli(CLI_SRC)
if "Commands" not in CLI:
    fail("could not parse the CLI command tree from crates/zalkanes-cli/src/main.rs")

CMD_LANGS = {"bash", "sh", "shell", "console", "zsh"}
ZALK_RE = re.compile(r"(?<![\w/.-])zalkanes\s+")


def extract_zalkanes_cmds(text):
    for lang, body in fenced_blocks(text):
        if lang not in CMD_LANGS:
            continue
        for line in joined_lines(body):
            line = re.sub(r"^\$\s+", "", line)
            m = ZALK_RE.search(line)
            if not m:
                continue
            cmd = line[m.start():]
            cmd = re.split(r"\s#\s|\s(?:\||>|2>|&&|;)\s?|\)$|\s*\)\s*$", cmd)[0]
            yield cmd.strip()


def check_cmd(where, cmd):
    try:
        toks = shlex.split(cmd)
    except ValueError:
        toks = cmd.split()
    if not toks or toks[0] != "zalkanes":
        return
    node, tree, path = None, CLI.get("Commands", {}), []
    i = 1
    while i < len(toks) and not toks[i].startswith("-") and tree is not None:
        if toks[i] not in tree:
            if node is None or node["sub"] is not None:
                fail(f"{where}: unknown zalkanes subcommand `{' '.join(path + [toks[i]])}` in: {cmd}")
                return
            break
        node = tree[toks[i]]
        path.append(toks[i])
        tree = CLI.get(node["sub"]) if node["sub"] else None
        i += 1
    if node is None or node["sub"] is not None:
        if not any(t in ("--help", "-h") for t in toks[1:]):
            fail(f"{where}: incomplete zalkanes command `{' '.join(path)}` in: {cmd}")
        return
    positionals = 0
    while i < len(toks):
        t = toks[i]
        if t in ("--help", "-h"):
            i += 1
            continue
        if t.startswith("--"):
            flag, has_eq = (t.split("=", 1)[0], "=" in t)
            if flag not in node["flags"]:
                fail(f"{where}: `zalkanes {' '.join(path)}` has no flag {flag} in: {cmd}")
                return
            if node["flags"][flag] and not has_eq:
                i += 1
        else:
            positionals += 1
        i += 1
    if positionals > node["positionals"]:
        fail(f"{where}: `zalkanes {' '.join(path)}` takes {node['positionals']} positional(s), "
             f"example passes {positionals}: {cmd}")


for md in current_docs():
    for cmd in extract_zalkanes_cmds(md.read_text(encoding="utf-8")):
        check_cmd(rel(md), cmd)

# ── 4. quickstart <-> acceptance script parity ──────────────────────────────
QUICK = ROOT / "docs/developers/quickstart.md"
SCRIPT = ROOT / "scripts/dev-quickstart-test.sh"
FLOW_PREFIXES = ("zalkanes ", "cargo build", "./scripts/run-regtest.sh", "eval ", "export PATH")
script_lines = set()
for line in joined_lines(SCRIPT.read_text(encoding="utf-8")):
    line = re.split(r"\s(?:\||>|2>)\s?", line)[0].strip()
    inner = re.fullmatch(r"\w+=\$\((.*)\)", line)  # VAR=$(command)
    if inner:
        line = inner.group(1).strip()
    script_lines.add(line)
for lang, body in fenced_blocks(QUICK.read_text(encoding="utf-8")):
    if lang not in CMD_LANGS:
        continue
    for line in joined_lines(body):
        if not line.startswith(FLOW_PREFIXES) or "<paste" in line:
            continue
        if line not in script_lines:
            fail(f"docs/developers/quickstart.md command is not run verbatim by scripts/dev-quickstart-test.sh: {line}")

# ── manifest + constants ────────────────────────────────────────────────────
MANIFEST = ROOT / "protocol/v0.toml"
manifest_bytes = MANIFEST.read_bytes()
MANIFEST_HASH = hashlib.sha256(manifest_bytes).hexdigest()


def parse_flat_toml(text):
    sections, cur = {}, None
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip() if not raw.strip().startswith("#") else ""
        if not line:
            continue
        if line.startswith("["):
            cur = line.strip("[]").strip()
            sections[cur] = {}
            continue
        k, v = [x.strip() for x in line.split("=", 1)]
        sections[cur][k] = v
    return sections


TOML = parse_flat_toml(manifest_bytes.decode("utf-8"))
CONSENSUS = (ROOT / "crates/zalkanes-core/src/consensus.rs").read_text(encoding="utf-8")


def rust_const(name):
    m = re.search(rf"pub const {name}:\s*[^=]+=\s*([^;]+);", CONSENSUS)
    return m.group(1).strip() if m else None


def rust_number(expr):
    expr = expr.split("//", 1)[0].strip().replace("_", "")
    if not re.fullmatch(r"[0-9\s*+()-]+", expr):
        return None
    return eval(expr, {"__builtins__": {}})  # arithmetic on digits only


# ── 5. manifest hash where stated ───────────────────────────────────────────
for site in MANIFEST_HASH_SITES:
    if MANIFEST_HASH not in (ROOT / site).read_text(encoding="utf-8"):
        fail(f"{site}: does not state the current protocol manifest hash {MANIFEST_HASH}")
ATTEST = json.loads((ROOT / "audit/gate-attestations.json").read_text(encoding="utf-8"))
if ATTEST["candidate"]["protocol_manifest_sha256"] != MANIFEST_HASH:
    fail("audit/gate-attestations.json candidate.protocol_manifest_sha256 != SHA-256(protocol/v0.toml)")

# ── 6/7. activation heights ─────────────────────────────────────────────────
TESTNET_MD = (ROOT / "docs/developers/testnet.md").read_text(encoding="utf-8")
m_testnet = TOML["network"]["testnet_activation_height"]
rs_testnet = rust_const("TESTNET_ACTIVATION_HEIGHT") or ""
rs_testnet_num = re.sub(r"[^0-9]", "", rs_testnet)
if m_testnet != rs_testnet_num:
    fail(f"testnet activation drift: manifest {m_testnet} vs consensus.rs {rs_testnet}")
doc_act = re.search(r"activation height:\s*([0-9,_]+)", TESTNET_MD)
if not doc_act or doc_act.group(1).replace(",", "").replace("_", "") != m_testnet:
    fail(f"docs/developers/testnet.md must state `activation height: {int(m_testnet):,}`")
if TOML["network"]["mainnet_activation_height"] != '"None"':
    fail("protocol/v0.toml: mainnet_activation_height must be \"None\"")
if (rust_const("MAINNET_ACTIVATION_HEIGHT") or "") != "None":
    fail("consensus.rs: MAINNET_ACTIVATION_HEIGHT must be None")
if not re.search(r"^mainnet:\s+disabled", TESTNET_MD, re.M):
    fail("docs/developers/testnet.md must state `mainnet: disabled`")
if not re.search(r"[Mm]ainnet.*disabled", (ROOT / "README.md").read_text(encoding="utf-8")):
    fail("README.md must say mainnet is disabled")

# ── 8. candidate identity ───────────────────────────────────────────────────
doc_cand = re.search(r"^candidate:\s+(\S+)", TESTNET_MD, re.M)
cand = ATTEST["candidate"]
if not doc_cand or doc_cand.group(1) != cand["tag"]:
    fail(f"docs/developers/testnet.md must state `candidate: {cand['tag']}`")
if not NO_GIT:
    def git(*args):
        r = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
        return r.stdout.strip() if r.returncode == 0 else None
    commit = git("rev-list", "-n1", cand["tag"])
    tag_obj = git("rev-parse", cand["tag"])
    if commit is None:
        fail(f"git tag {cand['tag']} not found locally (run `git fetch --tags`, or pass --no-git)")
    else:
        if commit != cand["commit"]:
            fail(f"tag {cand['tag']} points at {commit}, metadata says {cand['commit']}")
        if tag_obj != cand["tag_object"]:
            fail(f"tag object {tag_obj} != recorded {cand['tag_object']}")

# ── 9. testnet status line ──────────────────────────────────────────────────
active = ATTEST["gates"]["fresh_frozen_rc_testnet_activation"]["satisfied"] is True
expected_status = "ACTIVE" if active else "NOT YET ACTIVE"
st = re.search(r"^Network status:\s+(.+?)\s*$", TESTNET_MD, re.M)
if not st or st.group(1) != expected_status:
    fail(f"docs/developers/testnet.md must state `Network status:    {expected_status}` "
         f"(gate fresh_frozen_rc_testnet_activation.satisfied={active})")

# ── 10. generated limits table ──────────────────────────────────────────────
LIMITS_MD = ROOT / "docs/developers/protocol-limits.md"
BEGIN, END = "<!-- generated:protocol-limits:begin -->", "<!-- generated:protocol-limits:end -->"


def render_limits():
    rows = ["Source: `protocol/v0.toml`, SHA-256 `" + MANIFEST_HASH + "`.", "",
            "| section | key | value |", "|---|---|---|"]
    for section in ("limits", "carrier", "wasm", "network"):
        for k, v in TOML[section].items():
            rows.append(f"| `{section}` | `{k}` | `{v.strip(chr(34))}` |")
    return "\n".join(rows)


text = LIMITS_MD.read_text(encoding="utf-8")
if BEGIN not in text or END not in text:
    fail("docs/developers/protocol-limits.md is missing the generated-block markers")
else:
    head, rest = text.split(BEGIN, 1)
    _, tail = rest.split(END, 1)
    want = f"{BEGIN}\n{render_limits()}\n{END}"
    have = BEGIN + rest.split(END, 1)[0] + END
    if have != want:
        if FIX:
            LIMITS_MD.write_text(head + want + tail, encoding="utf-8")
            print("regenerated the limits table in docs/developers/protocol-limits.md")
            text = LIMITS_MD.read_text(encoding="utf-8")
        else:
            fail("docs/developers/protocol-limits.md limits table is stale; run scripts/check-docs.sh --fix")
    prose = text.split(END, 1)[1]
    for k in TOML["limits"]:
        if f"`{k}`" not in prose:
            fail(f"docs/developers/protocol-limits.md does not explain `{k}`")
for k, v in TOML["limits"].items():
    rs = rust_const(k.upper())
    if rs is None:
        fail(f"consensus.rs has no constant {k.upper()} for manifest key {k}")
        continue
    if rust_number(rs) != int(v):
        fail(f"consensus.rs {k.upper()} = {rs} but protocol/v0.toml {k} = {v}")

# ── 11. host imports documented ─────────────────────────────────────────────
RUNTIME = (ROOT / "crates/zalkanes-runtime/src/lib.rs").read_text(encoding="utf-8")
hi = re.search(r"const HOST_IMPORTS: \[&str; \d+\] = \[(.*?)\];", RUNTIME, re.S)
imports = re.findall(r'"(\w+)"', hi.group(1)) if hi else []
if not imports:
    fail("could not parse HOST_IMPORTS from crates/zalkanes-runtime/src/lib.rs")
for doc in ("docs/developers/contract-model.md", "docs/developers/wasm-rules.md"):
    body = (ROOT / doc).read_text(encoding="utf-8")
    for name in imports:
        if name not in body:
            fail(f"{doc}: host import `{name}` is not documented")

# ── report ──────────────────────────────────────────────────────────────────
if errors:
    print("documentation check FAILED:")
    for e in errors:
        print("  - " + e)
    sys.exit(1)
print(f"documentation check OK (manifest {MANIFEST_HASH[:16]}…, candidate {cand['tag']}, "
      f"testnet {expected_status}, {len(imports)} host imports)")
