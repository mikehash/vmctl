#!/usr/bin/env python3
"""Generate the VMCTL Technical Whitepaper & Roadmap PDF (TUI + Desktop)."""
from reportlab.lib.pagesizes import letter
from reportlab.lib.units import inch
from reportlab.lib import colors
from reportlab.lib.styles import getSampleStyleSheet, ParagraphStyle
from reportlab.platypus import (
    SimpleDocTemplate, Paragraph, Spacer, Table, TableStyle, PageBreak, HRFlowable
)

OUT = "/home/mike/.zeus/workspace/vmctl-tui/docs/vmctl-whitepaper-roadmap.pdf"

styles = getSampleStyleSheet()
H1 = ParagraphStyle("H1", parent=styles["Heading1"], fontSize=18, spaceAfter=10, textColor=colors.HexColor("#1a2b4a"))
H2 = ParagraphStyle("H2", parent=styles["Heading2"], fontSize=13, spaceBefore=12, spaceAfter=6, textColor=colors.HexColor("#2c4a7c"))
H3 = ParagraphStyle("H3", parent=styles["Heading3"], fontSize=11, spaceBefore=8, spaceAfter=4, textColor=colors.HexColor("#3a5a8c"))
BODY = ParagraphStyle("BODY", parent=styles["BodyText"], fontSize=9.5, leading=13, spaceAfter=5)
SMALL = ParagraphStyle("SMALL", parent=styles["BodyText"], fontSize=8.5, leading=11, textColor=colors.HexColor("#555555"))
BULLET = ParagraphStyle("BULLET", parent=BODY, leftIndent=14, bulletIndent=4, spaceAfter=3)
MONO = ParagraphStyle("MONO", parent=styles["BodyText"], fontName="Courier", fontSize=8.5, leading=11, leftIndent=10, textColor=colors.HexColor("#333333"))

def cell(text, style=SMALL):
    return Paragraph(text, style)

def tbl(data, widths, header=True):
    t = Table(data, colWidths=widths, repeatRows=1 if header else 0)
    style = [
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("GRID", (0, 0), (-1, -1), 0.4, colors.HexColor("#c8d0dc")),
        ("LEFTPADDING", (0, 0), (-1, -1), 5),
        ("RIGHTPADDING", (0, 0), (-1, -1), 5),
        ("TOPPADDING", (0, 0), (-1, -1), 3),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 3),
    ]
    if header:
        style += [
            ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#2c4a7c")),
            ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
            ("FONTNAME", (0, 0), (-1, 0), "Helvetica-Bold"),
            ("FONTSIZE", (0, 0), (-1, 0), 8.5),
        ]
    t.setStyle(TableStyle(style))
    return t

story = []

# ---- Title ----
story.append(Paragraph("VMCTL — Technical Whitepaper &amp; Roadmap", H1))
story.append(Paragraph("vm-bhyve front-ends: Desktop (Slint) + Terminal (ratatui) over one shared core", BODY))
story.append(Paragraph("Version 0.1 · October 2026 · Status: foundation landed, design locked", SMALL))
story.append(HRFlowable(width="100%", thickness=1, color=colors.HexColor("#2c4a7c"), spaceBefore=6, spaceAfter=10))

# ---- 1. Vision ----
story.append(Paragraph("1. Vision &amp; Design Principles", H2))
story.append(Paragraph(
    "VMCTL is a pair of front-ends for <b>vm-bhyve</b> (FreeBSD's bhyve manager) that make virtualization "
    "<b>powerful and easy</b> — the VirtualBox experience, on a real hypervisor. Two front-ends, one core: "
    "anything you do in the Desktop app shows up in the Terminal app and vice-versa, because both drive the "
    "same <b>vmctl-core</b> state and the same <b>vm(8)</b> commands.", BODY))
for b in [
    "<b>One core, two front-ends.</b> Desktop (Slint) and Terminal (ratatui) are thin shells over a shared Rust core. No duplicated logic, no drift.",
    "<b>Honest by construction.</b> Every action maps to a real <b>vm(8)</b> subcommand. Exit codes and stderr are surfaced verbatim — never swallowed, never faked.",
    "<b>Safe by default.</b> Plan-before-run for anything destructive. Root gate enforced. Lock semantics respected (a machine locked by another host is never touched without explicit confirmation).",
    "<b>Plain-English assistant.</b> Natural language → intent → validated plan → confirm → execute. The assistant proposes; the human confirms; the core executes.",
    "<b>Grounded in the real tool.</b> The design system, feature IDs, and command surface are taken from the working prototype (<i>vmctl-prototype.tsx</i>) and the live vm-bhyve 1.7.5 install — not invented.",
]:
    story.append(Paragraph(b, BULLET))

# ---- 2. Architecture ----
story.append(Paragraph("2. Architecture", H2))
story.append(Paragraph(
    "The system is layered so that every capability lives in exactly one place and both front-ends reuse it.", BODY))
story.append(tbl([
    [cell("<b>Layer</b>", SMALL), cell("<b>Responsibility</b>", SMALL), cell("<b>Notes</b>", SMALL)],
    [cell("vmctl-core", SMALL), cell("Typed <b>vm(8)</b> wrapper + VM model + actions + state machine + assistant engine. Honest exit codes.", SMALL), cell("Scaffolded: <i>vmbhyve.rs</i>, <i>actions.rs</i>", SMALL)],
    [cell("Backend trait", SMALL), cell("Abstract VM ops (list/create/start/stop/…). vm-bhyve is the first implementation.", SMALL), cell("Future: VirtualBox, QEMU, cloud", SMALL)],
    [cell("Assistant engine", SMALL), cell("Plain-English → intent → validated action plan → confirm → execute. Sequential runner stops at first non-zero exit and keeps real stderr.", SMALL), cell("Shared by TUI + Desktop", SMALL)],
    [cell("Desktop (Slint)", SMALL), cell("Windowed app: machines, snapshots, switches/storage, activity, assistant drawer, creation wizard.", SMALL), cell("Mission Control palette", SMALL)],
    [cell("Terminal (ratatui)", SMALL), cell("Panes: VM list, action bar, wizard, config editor, console attach, log tail.", SMALL), cell("ca0e750: list + actions done", SMALL)],
], [1.2*inch, 3.6*inch, 1.7*inch]))

story.append(Paragraph("2.1 Shared core contract", H3))
story.append(Paragraph(
    "Both front-ends call the same core API. The core owns: (a) the typed wrapper that shells out to <b>vm(8)</b> "
    "and parses its tabular output (including multi-word states like <i>Locked (nas01)</i> and <i>Running (4182)</i>); "
    "(b) the VM model and action layer (Start/Stop/Restart/Suspend/Resume/PowerOff/Snapshot/Rollback/Clone/Destroy/"
    "Create/Install/Console/Unlock); (c) the assistant planner that turns a sentence into a validated plan with "
    "risk/touches/memory-after metadata and per-step undo; (d) the sequential plan executor that stops at the first "
    "failure and preserves the real stderr.", BODY))

# ---- 3. Design system ----
story.append(Paragraph("3. Design System", H2))
story.append(Paragraph(
    "The <b>Mission Control</b> palette: crimson is action, ember is focus/assistant, and state colours never "
    "reuse crimson. Type stack: Orbitron (display), Rajdhani (UI), JetBrains Mono (data/commands).", BODY))
story.append(tbl([
    [cell("<b>Token</b>", SMALL), cell("<b>Value</b>", SMALL), cell("<b>Role</b>", SMALL)],
    [cell("crimson", SMALL), cell("#DC2F45", MONO), cell("Primary action, destructive emphasis", SMALL)],
    [cell("ember", SMALL), cell("#FF7A3D", MONO), cell("Focus, assistant, suggested next step", SMALL)],
    [cell("green", SMALL), cell("#34D399", MONO), cell("Running / success", SMALL)],
    [cell("amber", SMALL), cell("#F5B53D", MONO), cell("Suspended / warning", SMALL)],
    [cell("violet", SMALL), cell("#B79CFF", MONO), cell("Locked (held by another host)", SMALL)],
    [cell("bg / panel / surface", SMALL), cell("#0A0B0D / #0F1114 / #14171B", MONO), cell("Surfaces", SMALL)],
], [1.4*inch, 2.2*inch, 2.9*inch]))

# ---- 4. Feature spec ----
story.append(PageBreak())
story.append(Paragraph("4. Feature Specification", H2))
story.append(Paragraph("4.1 Assistant (F-401 … F-405)", H3))
for b in [
    "<b>F-401 Intent parsing</b> — plain English → intent (create/start/stop/snapshot/clone/explain/…).",
    "<b>F-402 Validated plan</b> — every plan shows steps, exact <b>vm(8)</b> commands, risk, what it touches, memory-after, and per-step undo.",
    "<b>F-403 Suggested next step</b> — each machine surfaces a context-aware default action (Running → Open console; Stopped → Start; Suspended → Resume; Locked → Explain lock).",
    "<b>F-404 Sequential execution</b> — run the plan step-by-step; stop at the first non-zero exit; keep the real stderr; nothing after the failure runs.",
    "<b>F-405 Explain errors</b> — surface the actual vm(8) stderr and explain the cause (e.g. a run.lock held by another host) without touching anything.",
]:
    story.append(Paragraph(b, BULLET))

story.append(Paragraph("4.2 VM lifecycle &amp; inventory", H3))
story.append(Paragraph(
    "List (live <b>vm list</b>), create (template → cpu/memory/disk/loader/ISO), install, start, stop, restart, "
    "suspend, resume, poweroff, destroy, clone. Templates: Ubuntu, FreeBSD, Debian, Windows, OpenBSD, Alpine — "
    "each with sensible defaults for cpu/mem/disk/loader/ISO.", BODY))

story.append(Paragraph("4.3 Snapshots &amp; images", H3))
story.append(Paragraph(
    "Snapshot (with <b>-f</b> for a live/crash-consistent snapshot of a running guest), rollback (stopped only), "
    "clone-from-snapshot, and snapshot listing with size/when. The assistant flags that a <b>-f</b> snapshot of a "
    "running disk is like a power cut — fine for rollback, and offers a quiesce-first alternative.", BODY))

story.append(Paragraph("4.4 Network &amp; storage", H3))
story.append(Paragraph(
    "Switches (standard / standard-nat / private) with plain-English descriptions; add NIC to switch; add disk "
    "(zvol) to a machine; datastores (ZFS datasets) with used/free. The assistant explains effects in guest terms "
    "(e.g. \"it appears as vtnet1 after a restart\").", BODY))

story.append(Paragraph("4.5 Safety model", H3))
for b in [
    "<b>Root gate</b> — vm(8) needs root; without it the app is read-only and says so, with the exact command to retry (<i>doas vmctl-desktop</i>).",
    "<b>Lock semantics</b> — a machine with a run.lock held by another host is never started/unlocked without explicit human confirmation that the other host is gone.",
    "<b>Plan-before-run</b> — destructive or multi-step actions always show the plan and wait for confirmation.",
    "<b>Honest exit codes</b> — every command's real exit code and stderr are shown; failures stop the plan.",
]:
    story.append(Paragraph(b, BULLET))

# ---- 5. Roadmap ----
story.append(Paragraph("5. Roadmap", H2))
story.append(tbl([
    [cell("<b>Milestone</b>", SMALL), cell("<b>Scope</b>", SMALL), cell("<b>Exit criteria</b>", SMALL), cell("<b>Status</b>", SMALL)],
    [cell("<b>M0 Foundation</b>", SMALL), cell("Crate + typed vm(8) wrapper + VM list + action layer.", SMALL), cell("Compiles; 6 tests green; live <b>vm list</b> parsed (testvm).", SMALL), cell("<b>Done</b> — 0c18a73, ca0e750", SMALL)],
    [cell("<b>M1 Core + Assistant</b>", SMALL), cell("Full action layer, assistant planner + sequential runner, plan cards.", SMALL), cell("planFor covers create/start/stop/snapshot/explain; runner stops at first failure with real stderr.", SMALL), cell("Next", SMALL)],
    [cell("<b>M2 Desktop shell</b>", SMALL), cell("Slint app: machines/snapshots/network/activity views + assistant drawer + wizard.", SMALL), cell("Parity with prototype; anything done here shows in TUI.", SMALL), cell("Planned", SMALL)],
    [cell("<b>M3 TUI panes</b>", SMALL), cell("ratatui: list + actions + wizard + config editor + console attach + log tail.", SMALL), cell("Feature parity with Desktop for core flows.", SMALL), cell("Planned", SMALL)],
    [cell("<b>M4 Polish &amp; hardening</b>", SMALL), cell("Error UX, lock flows, docs, packaging, cross-host awareness.", SMALL), cell("Safe-by-default verified; docs published.", SMALL), cell("Planned", SMALL)],
], [1.3*inch, 2.4*inch, 2.2*inch, 1.1*inch]))

# ---- 6. Risks ----
story.append(Paragraph("6. Risks &amp; Mitigations", H2))
for b in [
    "<b>vm(8) output drift</b> — parsing is pinned to vm-bhyve 1.7.5 and covered by unit tests (including multi-word states); a version bump is a test change, not a silent break.",
    "<b>Root requirement</b> — surfaced honestly as a read-only mode with the exact retry command, never a crash.",
    "<b>Destructive actions</b> — plan-before-run + per-step undo + stop-at-first-failure; locks respected.",
    "<b>Two front-ends drifting</b> — impossible by construction: both are thin shells over one core.",
]:
    story.append(Paragraph(b, BULLET))

story.append(Spacer(1, 10))
story.append(HRFlowable(width="100%", thickness=0.5, color=colors.HexColor("#c8d0dc"), spaceBefore=6, spaceAfter=6))
story.append(Paragraph(
    "Generated from the working prototype (<i>vmctl-prototype.tsx</i>) and the live vm-bhyve 1.7.5 command surface. "
    "Foundation commits: 0c18a73 (crate + wrapper), ca0e750 (VM-list TUI + action layer).", SMALL))

doc = SimpleDocTemplate(OUT, pagesize=letter,
                        leftMargin=0.7*inch, rightMargin=0.7*inch,
                        topMargin=0.7*inch, bottomMargin=0.7*inch)
doc.build(story)
print("WROTE", OUT)
