#!/usr/bin/env python3
"""Drive the Draft panel through its self-test and check what the agent would see.

Needs a built debug binary on macOS (cargo build --profile native-dev in
crates/monocode-gpui). Opens a window for a few seconds.
"""
import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--binary",
    type=pathlib.Path,
    default=root / "crates/monocode-gpui/target/native-dev/monocode-gpui",
)
parser.add_argument("--keep", action="store_true", help="keep the output directory")
args = parser.parse_args()
if sys.platform != "darwin":
    sys.exit("The Draft panel is macOS only")
if not args.binary.exists():
    sys.exit(f"Build first: {args.binary} does not exist")

out = pathlib.Path(tempfile.mkdtemp(prefix="monocode-draft-verify-"))
page = out / "page.html"
shutil.copy(root / "docs/native/examples/draft-spike.html", page)
actions = [
    {"draft": "$draft", "action": {"kind": "click", "reference": "@Add one"}},
    {"draft": "$draft", "action": {"kind": "click", "reference": "@Your name"}},
    {"draft": "$draft", "action": {"kind": "type", "text": "Ada", "replace": True}},
    {"debug": "takeover"},
    {"draft": "$draft", "action": {"kind": "click", "reference": "@Add one"}},
    {"draft": "$draft", "action": {"kind": "resume"}},
    {"draft": "$draft", "width": 375, "height": 700},
    {"debug": "note"},
    {"draft": "$draft", "action": {"kind": "wait", "seconds": 2}},
    {"debug": "send_later"},
    {"draft": "$draft", "action": {"kind": "wait", "seconds": 6}},
    {"draft": "$draft", "action": {"kind": "keep", "title": "Spike"}},
    {"draft": "$draft", "action": {"kind": "close"}},
]


def run(actions, source=page):
    # The draft is drawn inside a thread, as a card between two messages.
    env = dict(
        os.environ,
        MONOCODE_DRAFT_SELFTEST=str(source),
        MONOCODE_DRAFT_SELFTEST_THREAD="1",
        MONOCODE_DRAFT_SELFTEST_PANEL="900",
        MONOCODE_DRAFT_ACTIONS=json.dumps(actions),
    )
    subprocess.run([str(args.binary)], env=env, check=True, timeout=120,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    results = []
    n = 0
    while (path := out / f"{source.name}.{n}.json").exists():
        results.append(json.loads(path.read_text()))
        n += 1
    return results


steps = run(actions)
refs = {e["name"]: e["ref"] for e in steps[0]["elements"]}

failures = []


def check(condition, message):
    if not condition:
        failures.append(message)


opening = steps[0]
check(opening.get("loaded") is True, "the page loads")
check(opening["diagnostics"]["ready"] is True, "diagnostics report ready")
check(opening["width"] >= 1000 and (out / "page.html.0.png").stat().st_size > 5000, "a real picture comes back")
check("ready" in [c["text"] for c in opening["console"]], "console output is captured")
check("Add one" in refs and "Your name" in refs, "controls are listed with refs")
placed = opening.get("placed") or [0, 0, 0, 0, 1]
check(placed[4] == 0 and placed[2] > 300, "the page is shown inside the thread")
check(abs(placed[3] - opening["contentHeight"]) <= 2, "the card is as tall as the page")
check("1" in steps[1]["text"].split("Add one")[1][:4], "a click reaches the page")
check("Hello, Ada" in steps[3]["text"], "typing replaces the input value")
check("trusted clicks: 0" in steps[3]["text"], "agent clicks are not trusted events")
check(steps[4].get("takeover") is True, "the test person takes over")
check("error" in steps[5] and "hand" in steps[5]["error"].lower(), "the agent is gated while the person drives")
check(steps[6].get("userControl") is False, "resume hands the page back")
check(steps[7]["width"] == 750 and steps[7]["height"] == 1400, "a resize relays the page out")
check(steps[8].get("pending") == 1, "a note pinned in the page is collected, not yet sent")
timed_out = steps[9]
check(timed_out.get("event") == "timeout" and timed_out["feedback"] == [], "a wait with nothing sent times out and carries no notes")
woken = steps[11]
check(woken.get("event") == "feedback", "sending wakes a waiting agent")
check([f["note"] for f in woken.get("feedback", [])] == ["Make this blue"], "the agent receives the pinned note")
check(timed_out.get("userActions") == ['clicked "Add one"'], "the agent learns what the person did by hand")
check(not any("Low contrast" in issue for issue in opening.get("issues", [])), "readable text is not flagged")
check(any("without a label" in issue for issue in opening.get("issues", [])), "the page is audited for missing labels")
kept = steps[12].get("kept")
check(bool(kept) and pathlib.Path(kept["png"]).exists() and pathlib.Path(kept["html"]).exists(), "keep stages the picture and the page")
check("closed" in steps[13], "close frees the draft")

# The optional controls kit: controls outside the content, bound to mc.state.
controls_page = out / "controls.html"
shutil.copy(root / "docs/native/examples/controls-kit-all.html", controls_page)
controls = run([
    {"draft": "$draft", "action": {"kind": "click", "reference": "@Uppercase"}},
    {"draft": "$draft", "action": {"kind": "click", "reference": "@Two"}},
    {"draft": "$draft", "action": {"kind": "click", "reference": "@center"}},
    {"draft": "$draft", "action": {"kind": "click", "reference": "@Increase Size"}},
    {"draft": "$draft", "action": {"kind": "click", "reference": "@Reset"}},
], controls_page)
check(controls[0]["console"] == [] and controls[0]["issues"] == [], "the controls kit renders with no console errors or audit issues")
check(controls[0].get("state", {}).get("size") == 14, "the agent sees the page's control values")
check(controls[1]["state"]["caps"] is True, "a checkbox changes the state")
check(controls[2]["state"]["mode"] == "b" and controls[3]["state"]["align"] == "center", "segmented and radio controls change the state")
check(controls[4]["state"]["size"] == 15, "a stepper steps")
check(controls[5]["state"] == controls[0]["state"], "a button resets the state")

if failures:
    print("FAILED:\n  " + "\n  ".join(failures))
    print(f"Output kept in {out}")
    sys.exit(1)
print("Draft panel verified")
if args.keep:
    print(f"Output in {out}")
else:
    shutil.rmtree(out)
