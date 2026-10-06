// scripts/setup-release-secrets.sh (ADR-0019) against fake `op`, `gh` and
// Sparkle `generate_keys` on PATH: no test reads 1Password or the keychain or
// sets a GitHub secret. The certificates are throwaway self-signed ones.
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

setDefaultTimeout(60_000);

const repoRoot = resolve(import.meta.dir, "../..");
const script = join(repoRoot, "scripts", "setup-release-secrets.sh");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

const CERT_ITEM =
  "Polygloss · Apple Developer ID Application (Dak Washbrook, 5U7E4UQ5M3)";
const API_ITEM =
  "Polygloss · App Store Connect API key (notarization, 87T26G474H)";
const SPARKLE_ITEM = "Polygloss · Sparkle EdDSA update signing key";
const IDENTITY = "Developer ID Application: Test Person (5U7E4UQ5M3)";
const P12_PASSWORD = "p12-SECRET-password-4711";
const KEY_ID = "ABC123DEF4";
const ISSUER = "69a6de70-1234-47e3-e053-5b8c7c11a4d1";
const P8_SECRET_LINE =
  "MIGTAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBHkwdwIBAQQgFAKEFAKEFAKE";
const P8 = `-----BEGIN PRIVATE KEY-----\n${P8_SECRET_LINE}\n-----END PRIVATE KEY-----\n`;
const SPARKLE_PRIVATE = "c2VjcmV0LXNwYXJrbGUta2V5LWZvci10ZXN0cy0wMTIz";
const SPARKLE_PUBLIC = Buffer.alloc(32, 9).toString("base64");
const OLD_SPARKLE_PRIVATE = "b2xkLXNwYXJrbGUta2V5LWluLTFwYXNzd29yZC00NTY=";
const OLD_SPARKLE_PUBLIC = Buffer.alloc(32, 7).toString("base64");
const SECRETS = [
  "APPLE_CERTIFICATE",
  "APPLE_CERTIFICATE_PASSWORD",
  "APPLE_SIGNING_IDENTITY",
  "APPLE_API_KEY",
  "APPLE_API_ISSUER",
  "APPLE_API_PRIVATE_KEY",
  "SPARKLE_PRIVATE_ED_KEY",
];

let n = 0;
function scratch(name: string): string {
  const dir = join(sandbox.home, `${name}-${n++}`);
  mkdirSync(dir, { recursive: true });
  return dir;
}

function must(argv: string[]): void {
  const r = Bun.spawnSync(argv, { env: sandbox.env });
  if (r.exitCode !== 0)
    throw new Error(`${argv.join(" ")}: ${r.stderr.toString()}`);
}

/** A .p12 of a self-signed certificate `cn`/`ou` under P12_PASSWORD. */
function makeP12(cn: string, ou: string): Buffer {
  const dir = scratch("cert");
  must([
    "/usr/bin/openssl",
    "req",
    "-x509",
    "-newkey",
    "rsa:2048",
    "-nodes",
    "-days",
    "2",
    "-keyout",
    join(dir, "key.pem"),
    "-out",
    join(dir, "cert.pem"),
    "-subj",
    `/UID=${ou}/CN=${cn}/OU=${ou}/O=Test Person/C=US`,
  ]);
  must([
    "/usr/bin/openssl",
    "pkcs12",
    "-export",
    "-inkey",
    join(dir, "key.pem"),
    "-in",
    join(dir, "cert.pem"),
    "-out",
    join(dir, "id.p12"),
    "-passout",
    `pass:${P12_PASSWORD}`,
  ]);
  return readFileSync(join(dir, "id.p12"));
}
const ourP12 = makeP12(IDENTITY, "5U7E4UQ5M3");

function writeExecutable(path: string, text: string): void {
  writeFileSync(path, text);
  chmodSync(path, 0o755);
}

// op: `item list` prints $FAKE_OP/items.json; `read op://dak.dev/<id>/<field>`
// prints (or writes to --out-file) $FAKE_OP/<id>/<field>, after
// $FAKE_OP_SLEEP seconds; `item create … -` keeps stdin. Every call's argv
// goes to $FAKE_LOG.
const fakeOp = `#!/bin/bash
printf 'op' >>"$FAKE_LOG"; printf ' [%s]' "$@" >>"$FAKE_LOG"; printf '\\n' >>"$FAKE_LOG"
case "$1 $2" in
  "item list") [ -z "\${FAKE_OP_SIGNED_OUT-}" ] || exit 1; cat "$FAKE_OP/items.json" ;;
  "item create") cat >"$FAKE_OP/created.json" ;;
  read*)
    sleep "\${FAKE_OP_SLEEP:-0}"
    out="" ref=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --out-file) out="$2"; shift 2 ;;
        op://*) ref="\${1#op://dak.dev/}"; shift ;;
        *) shift ;;
      esac
    done
    [ -f "$FAKE_OP/$ref" ] || { echo "[ERROR] no field $ref" >&2; exit 1; }
    if [ -n "$out" ]; then cp "$FAKE_OP/$ref" "$out" 2>/dev/null; else cat "$FAKE_OP/$ref"; fi ;;
  *) exit 1 ;;
esac
`;

// gh: `secret|variable set NAME` keeps stdin in $FAKE_GH/<kind>-NAME, failing
// for $FAKE_GH_FAIL.
const fakeGh = `#!/bin/bash
printf 'gh' >>"$FAKE_LOG"; printf ' [%s]' "$@" >>"$FAKE_LOG"; printf '\\n' >>"$FAKE_LOG"
[ "$3" != "\${FAKE_GH_FAIL-}" ] || exit 1
case "$1 $2" in
  "secret set" | "variable set") cat >"$FAKE_GH/$1-$3" ;;
  *) exit 1 ;;
esac
`;

// Sparkle's generate_keys over a keychain in $FAKE_SPARKLE.
const fakeGenerateKeys = `#!/bin/bash
printf 'generate_keys' >>"$FAKE_LOG"; printf ' [%s]' "$@" >>"$FAKE_LOG"; printf '\\n' >>"$FAKE_LOG"
[ "$1 $2" = "--account dev.dak.polygloss" ] || exit 1
case "\${3-}" in
  "")
    if [ ! -f "$FAKE_SPARKLE/public" ]; then
      printf '%s\\n' "${SPARKLE_PRIVATE}" >"$FAKE_SPARKLE/private"
      printf '%s\\n' "${SPARKLE_PUBLIC}" >"$FAKE_SPARKLE/public"
    fi
    echo "A key pair exists; the public key is in the keychain" ;;
  -p) cat "$FAKE_SPARKLE/public" ;;
  -x) cp "$FAKE_SPARKLE/private" "$4" ;;
esac
`;

type Setup = {
  p12?: Buffer;
  password?: string;
  sparkleItem?: boolean;
  apiItem?: boolean;
  args?: string[];
  env?: Record<string, string>;
};

/** The world the script runs in: fakes, 1Password items, a private TMPDIR. */
function world(opts: Setup = {}) {
  const root = scratch("world");
  const bin = join(root, "bin");
  const op = join(root, "op");
  const gh = join(root, "gh");
  const sparkle = join(root, "sparkle");
  const tmp = join(root, "tmp");
  for (const d of [bin, op, gh, sparkle, tmp]) mkdirSync(d);
  writeExecutable(join(bin, "op"), fakeOp);
  writeExecutable(join(bin, "gh"), fakeGh);
  writeExecutable(join(bin, "generate_keys"), fakeGenerateKeys);
  const items = [
    { id: "unrelated", title: "Polygloss · something else" },
    { id: "certid", title: CERT_ITEM },
  ];
  mkdirSync(join(op, "certid"));
  writeFileSync(
    join(op, "certid", "developer-id-application.p12"),
    opts.p12 ?? ourP12,
  );
  writeFileSync(
    join(op, "certid", "p12 password"),
    opts.password ?? P12_PASSWORD,
  );
  if (opts.apiItem !== false) {
    items.push({ id: "apiid", title: API_ITEM });
    mkdirSync(join(op, "apiid"));
    writeFileSync(join(op, "apiid", "Key ID"), KEY_ID);
    writeFileSync(join(op, "apiid", "Issuer ID"), ISSUER);
    writeFileSync(join(op, "apiid", `AuthKey_${KEY_ID}.p8`), P8);
  }
  if (opts.sparkleItem) {
    items.push({ id: "sparkleid", title: SPARKLE_ITEM });
    mkdirSync(join(op, "sparkleid"));
    writeFileSync(join(op, "sparkleid", "private key"), OLD_SPARKLE_PRIVATE);
    writeFileSync(join(op, "sparkleid", "public key"), OLD_SPARKLE_PUBLIC);
  }
  writeFileSync(join(op, "items.json"), JSON.stringify(items));
  const env = {
    ...sandbox.env,
    PATH: `${bin}:${process.env.PATH}`,
    TMPDIR: tmp,
    SPARKLE_BIN_DIR: bin,
    FAKE_LOG: join(root, "calls.log"),
    FAKE_OP: op,
    FAKE_GH: gh,
    FAKE_SPARKLE: sparkle,
    ...opts.env,
  };
  const calls = () =>
    existsSync(env.FAKE_LOG)
      ? readFileSync(env.FAKE_LOG, "utf8").split("\n").filter(Boolean)
      : [];
  const set = () =>
    Object.fromEntries(
      readdirSync(gh).map((f) => [f, readFileSync(join(gh, f), "utf8")]),
    );
  const created = () =>
    existsSync(join(op, "created.json"))
      ? (JSON.parse(readFileSync(join(op, "created.json"), "utf8")) as {
          title: string;
          category: string;
          fields: { label: string; type: string; value: string }[];
        })
      : null;
  return { env, tmp, calls, set, created };
}

function run(opts: Setup = {}) {
  const w = world(opts);
  const r = Bun.spawnSync(["bash", script, ...(opts.args ?? [])], {
    cwd: sandbox.home,
    env: w.env,
  });
  return {
    ...w,
    code: r.exitCode ?? -1,
    output: r.stdout.toString() + r.stderr.toString(),
  };
}

/**
 * Every secret value; none may reach an argv. The output does not name the
 * API key's ids either (the Key ID is in the item's title and the .p8's name,
 * so op's argv may).
 */
const SECRET_VALUES = [
  P12_PASSWORD,
  P8_SECRET_LINE,
  SPARKLE_PRIVATE,
  OLD_SPARKLE_PRIVATE,
  ourP12.toString("base64").slice(0, 40),
];
function expectNoSecretIn(
  text: string,
  also: string[] = [KEY_ID, ISSUER],
): void {
  for (const secret of [...SECRET_VALUES, ...also])
    expect({ secret, leaked: text.includes(secret) }).toEqual({
      secret,
      leaked: false,
    });
}

describe("scripts/setup-release-secrets.sh", () => {
  test("sets every secret and the variable from 1Password, saving a new Sparkle key there first", () => {
    const r = run();
    expect(r.output).toContain("setup-release-secrets: done");
    expect(r.code).toBe(0);
    const set = r.set();
    expect(Object.keys(set).sort()).toEqual(
      [
        ...SECRETS.map((s) => `secret-${s}`),
        "variable-SPARKLE_PUBLIC_ED_KEY",
      ].sort(),
    );
    expect(
      Buffer.from(set["secret-APPLE_CERTIFICATE"]!, "base64").equals(ourP12),
    ).toBe(true);
    expect(set["secret-APPLE_CERTIFICATE_PASSWORD"]).toBe(P12_PASSWORD);
    expect(set["secret-APPLE_SIGNING_IDENTITY"]).toBe(IDENTITY);
    expect(set["secret-APPLE_API_KEY"]).toBe(KEY_ID);
    expect(set["secret-APPLE_API_ISSUER"]).toBe(ISSUER);
    expect(set["secret-APPLE_API_PRIVATE_KEY"]).toBe(P8);
    expect(set["secret-SPARKLE_PRIVATE_ED_KEY"]).toBe(SPARKLE_PRIVATE);
    expect(set["variable-SPARKLE_PUBLIC_ED_KEY"]).toBe(SPARKLE_PUBLIC);

    // The new key pair is in 1Password, the private key concealed.
    const item = r.created();
    expect(item?.title).toBe(SPARKLE_ITEM);
    expect(item?.category).toBe("SECURE_NOTE");
    const field = (label: string) =>
      item?.fields.find((f) => f.label === label);
    expect(field("private key")).toMatchObject({
      type: "CONCEALED",
      value: SPARKLE_PRIVATE,
    });
    expect(field("public key")?.value).toBe(SPARKLE_PUBLIC);

    const calls = r.calls();
    // 1Password before GitHub, every op call on the right account, every
    // value on stdin.
    const firstGh = calls.findIndex((c) => c.startsWith("gh "));
    expect(calls.findLastIndex((c) => c.startsWith("op "))).toBeLessThan(
      firstGh,
    );
    for (const c of calls.filter((c) => c.startsWith("op ")))
      expect(c).toEndWith(" [--account] [my.1password.com]");
    expect(calls).toContain(
      "op [item] [create] [--vault] [dak.dev] [-] [--account] [my.1password.com]",
    );
    expect(calls.filter((c) => c.startsWith("gh "))).toEqual([
      ...SECRETS.map(
        (s) => `gh [secret] [set] [${s}] [--repo] [dakdevs/polygloss]`,
      ),
      "gh [variable] [set] [SPARKLE_PUBLIC_ED_KEY] [--repo] [dakdevs/polygloss]",
    ]);
    // Its own keychain account, so another app's Sparkle key is never used.
    expect(
      calls
        .filter((c) => c.startsWith("generate_keys"))
        .map((c) => c.replace(/ \[-x\] \[.*\]$/, " [-x] [<file>]")),
    ).toEqual([
      "generate_keys [--account] [dev.dak.polygloss]",
      "generate_keys [--account] [dev.dak.polygloss] [-p]",
      "generate_keys [--account] [dev.dak.polygloss] [-x] [<file>]",
    ]);
    expectNoSecretIn(calls.join("\n"), [ISSUER]);
    expectNoSecretIn(r.output);
    expect(readdirSync(r.tmp)).toEqual([]);
  });

  test("an existing Sparkle item is used as is: no key is made or saved", () => {
    const r = run({ sparkleItem: true, args: ["--repo", "someone/fork"] });
    expect(r.code).toBe(0);
    const set = r.set();
    expect(set["secret-SPARKLE_PRIVATE_ED_KEY"]).toBe(OLD_SPARKLE_PRIVATE);
    expect(set["variable-SPARKLE_PUBLIC_ED_KEY"]).toBe(OLD_SPARKLE_PUBLIC);
    expect(r.created()).toBeNull();
    expect(r.calls().filter((c) => c.startsWith("generate_keys"))).toEqual([]);
    for (const c of r.calls().filter((c) => c.startsWith("gh ")))
      expect(c).toEndWith(" [--repo] [someone/fork]");
    expectNoSecretIn(r.output);
  });

  test("refuses any certificate that is not team 5U7E4UQ5M3's Developer ID, before anything is made or set", () => {
    const cases: [Buffer, string][] = [
      [
        makeP12("Developer ID Application: Someone (FCSF68W94H)", "FCSF68W94H"),
        "refusing team FCSF68W94H",
      ],
      // The name claims our team; Apple's OU says otherwise.
      [makeP12(IDENTITY, "FCSF68W94H"), "refusing team FCSF68W94H"],
      [
        makeP12("Developer ID Application: Other (ZZZZZZZZZZ)", "ZZZZZZZZZZ"),
        "not team 5U7E4UQ5M3: refusing it",
      ],
      [
        makeP12("Apple Distribution: Test Person (5U7E4UQ5M3)", "5U7E4UQ5M3"),
        "is not a Developer ID Application certificate",
      ],
    ];
    for (const [p12, message] of cases) {
      const r = run({ p12 });
      expect({ message, code: r.code }).toEqual({ message, code: 1 });
      expect(r.output).toContain(message);
      expect(r.set()).toEqual({});
      expect(r.created()).toBeNull();
      expect(
        r
          .calls()
          .filter(
            (c) =>
              !c.startsWith("op [item] [list]") && !c.startsWith("op [read]"),
          ),
      ).toEqual([]);
      expectNoSecretIn(r.output);
      expect(readdirSync(r.tmp)).toEqual([]);
    }
  });

  test("--dry-run names what it would read and set, and reads nothing", () => {
    const r = run({ args: ["--dry-run"] });
    expect(r.code).toBe(0);
    expect(r.calls()).toEqual([]);
    expect(r.set()).toEqual({});
    for (const name of [
      ...SECRETS,
      "SPARKLE_PUBLIC_ED_KEY",
      CERT_ITEM,
      API_ITEM,
      SPARKLE_ITEM,
    ])
      expect(r.output).toContain(name);
    expect(r.output).toContain("variable SPARKLE_PUBLIC_ED_KEY");
    expect(readdirSync(r.tmp)).toEqual([]);
  });

  test("a missing item, a signed-out op or a wrong p12 password fails before GitHub", () => {
    const noApi = run({ apiItem: false });
    expect(noApi.code).toBe(1);
    expect(noApi.output).toContain(
      `no 1Password item "${API_ITEM}" in dak.dev`,
    );
    const signedOut = run({ env: { FAKE_OP_SIGNED_OUT: "1" } });
    expect(signedOut.code).toBe(1);
    expect(signedOut.output).toContain("op signin --account my.1password.com");
    const wrong = run({ password: "not-the-password" });
    expect(wrong.code).toBe(1);
    expect(wrong.output).toContain("does not open with its p12 password");
    for (const r of [noApi, signedOut, wrong]) {
      expect(r.set()).toEqual({});
      expect(r.calls().filter((c) => c.startsWith("gh "))).toEqual([]);
      expect(readdirSync(r.tmp)).toEqual([]);
    }
  });

  test("a failing gh call stops it and the temp files go", () => {
    const r = run({ env: { FAKE_GH_FAIL: "APPLE_API_ISSUER" } });
    expect(r.code).toBe(1);
    expect(r.output).toContain("gh secret set APPLE_API_ISSUER failed");
    expect(Object.keys(r.set())).not.toContain("secret-APPLE_API_PRIVATE_KEY");
    expect(readdirSync(r.tmp)).toEqual([]);
  });

  test("an interrupt removes the temp files too", async () => {
    const w = world({ env: { FAKE_OP_SLEEP: "1" } });
    const proc = Bun.spawn(["bash", script], { cwd: sandbox.home, env: w.env });
    // Interrupt while op writes the .p12 into the temp dir.
    const deadline = Date.now() + 30_000;
    while (
      !w.calls().some((c) => c.includes("[--out-file]")) &&
      Date.now() < deadline
    )
      await Bun.sleep(20);
    expect(readdirSync(w.tmp).length).toBe(1);
    proc.kill("SIGTERM");
    expect(await proc.exited).toBe(143);
    // The orphaned op finishes its sleep and finds no dir to write into.
    await Bun.sleep(1_500);
    expect(readdirSync(w.tmp)).toEqual([]);
    expect(w.set()).toEqual({});
  });

  test("usage errors exit 2", () => {
    for (const args of [["--nope"], ["--repo"], ["--repo", "not a repo"]])
      expect({ args, code: run({ args }).code }).toEqual({ args, code: 2 });
  });
});
