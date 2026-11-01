import assert from "node:assert/strict";
import test from "node:test";
import { compare, runEngine } from "../adapter-templates-diff.mjs";
import { ipcDenied } from "./ipc-guard.mjs";

function fixture(overrides = {}) {
  const messages = [];
  const calls = [];
  return {
    messages, calls,
    dependencies: {
      ipcProbe: async (kind) => {
        assert.equal(kind, "child-pipe");
        return { kind: "allowed" };
      },
      requireIpc: false,
      output: { log: (message) => messages.push(message), error: (message) => messages.push(message) },
      engine: (command, args, lines) => {
        calls.push({ command, args, lines });
        return lines.map(() => ({ match: true, title: "A title" }));
      },
      ...overrides,
    },
  };
}

for (const code of ["EPERM", "EACCES"]) {
  for (const requireIpc of [false, true]) {
    test(`${code} ${requireIpc ? "fails required CI" : "explicitly skips"} before either engine starts`, async () => {
      const f = fixture({ requireIpc, ipcProbe: async () => ({ kind: "denied", why: `child pipe: ${code}` }) });
      assert.equal(await compare({ seed: 7, count: 5000 }, f.dependencies), requireIpc ? 1 : 0);
      assert.equal(f.calls.length, 0);
      assert.equal(f.messages.length, 1);
      assert.match(f.messages[0], new RegExp(code));
      assert.match(f.messages[0], /no cases compared/);
      assert.match(f.messages[0], requireIpc ? /BRAUSER_REQUIRE_IPC_TESTS=1/ : /skipping/);
    });
  }
}

test("a failed probe fails rather than silently skipping", async () => {
  const f = fixture({ ipcProbe: async () => ({ kind: "failed", why: "child pipe: ETIMEDOUT" }) });
  assert.equal(await compare({ seed: 7, count: 1 }, f.dependencies), 1);
  assert.equal(f.calls.length, 0);
  assert.match(f.messages[0], /probe failed.*ETIMEDOUT/);
});

test("an allowed probe runs both engines in batches and reports all case counts", async () => {
  const f = fixture();
  assert.equal(await compare({ seed: 7, count: 2001 }, f.dependencies), 0);
  assert.deepEqual(f.calls.map(({ lines }) => lines.length), [2000, 2000, 1, 1]);
  assert.deepEqual(f.calls[0].args, ["adapters", "eval"]);
  assert.match(f.calls[1].args[0], /adapter-templates-eval\.mjs$/);
  assert.deepEqual(f.calls[0].lines, f.calls[1].lines);
  assert.deepEqual(f.calls[2].lines, f.calls[3].lines);
  assert.match(f.messages.at(-1), /seed 7, 2001 cases, 2001 matched \(2001 with a title\)/);
});

test("engine failures identify the seed and batch, with no passing comparison summary", async () => {
  for (const failingCall of [1, 2, 3]) {
    const f = fixture();
    const engine = f.dependencies.engine;
    f.dependencies.engine = (...args) => {
      if (f.calls.length + 1 === failingCall) throw new Error("engine timed out after 60000 ms");
      return engine(...args);
    };
    assert.equal(await compare({ seed: 7, count: 2001 }, f.dependencies), 1);
    assert.equal(f.calls.length, failingCall - 1);
    assert.match(f.messages.at(-1), failingCall === 3 ? /seed 7, cases 2000\.\.2000/ : /seed 7, cases 0\.\.1999/);
    assert.match(f.messages.at(-1), /timed out after 60000 ms/);
    assert.equal(f.messages.some((message) => /cases, .*matched/.test(message)), false);
  }
});

test("differences still fail with the exact reproduction index", async () => {
  const f = fixture();
  let calls = 0;
  f.dependencies.engine = () => [{ match: ++calls === 1 }];
  assert.equal(await compare({ seed: 7, count: 5000, only: "1234" }, f.dependencies), 1);
  assert.match(f.messages.join("\n"), /seed 7 case 1234 differs/);
  assert.match(f.messages.join("\n"), /1 of 1 cases differ/);
});

test("the generator usefulness guard still fails an all-no-match run", async () => {
  const f = fixture({ engine: (_command, _args, lines) => lines.map(() => ({ match: false, reason: "unrecognized" })) });
  assert.equal(await compare({ seed: 7, count: 500 }, f.dependencies), 1);
  assert.match(f.messages.at(-1), /too few matches/);
});

test("engine spawn is bounded and retains JSON Lines escaping", () => {
  const result = runEngine("host", ["adapters", "eval"], ['{"title":"a\u2028b\u2029c"}'], (command, args, options) => {
    assert.equal(command, "host");
    assert.deepEqual(args, ["adapters", "eval"]);
    assert.equal(options.timeout, 60_000);
    assert.equal(options.killSignal, "SIGKILL");
    assert.equal(options.input, '{"title":"a\\u2028b\\u2029c"}\n');
    return { status: 0, stdout: '{"match":true}\n', stderr: "" };
  });
  assert.deepEqual(result, [{ match: true }]);
});

test("timeout, denied pipes with zero status, and EPIPE are failures", () => {
  for (const [code, expected] of [["ETIMEDOUT", /host adapters eval: timed out after 60000 ms/], ["EPERM", /denied/], ["EPIPE", /engine crashed/]]) {
    assert.throws(() => runEngine("host", ["adapters", "eval"], ["{}"], () => ({
      status: 0, stdout: "", stderr: "engine crashed",
      error: Object.assign(new Error("denied"), { code }),
    })), expected);
  }
});

test("engine crashes, invalid JSON, and missing or extra results cannot pass", () => {
  for (const [result, expected] of [
    [{ status: 1, stderr: "bad input" }, /bad input/],
    [{ status: 0, stdout: "not JSON\n" }, SyntaxError],
    [{ status: 0, stdout: "{}\n" }, /expected 2 engine results, got 1/],
    [{ status: 0, stdout: "{}\n{}\n{}\n" }, /expected 2 engine results, got 3/],
  ]) {
    assert.throws(() => runEngine("host", [], ["{}", "{}"], () => result), expected);
  }
});

test("real child pipes round-trip through runEngine when the sandbox permits them", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  assert.deepEqual(runEngine(process.execPath, ["-e", "process.stdin.pipe(process.stdout)"], ['{"match":true}', '{"match":false}']), [
    { match: true }, { match: false },
  ]);
});
