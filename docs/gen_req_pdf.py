#!/usr/bin/env python3
"""Generate the vmctl app requirements PDF (TUI + Desktop)."""
from reportlab.lib.pagesizes import letter
from reportlab.lib.units import inch
from reportlab.lib import colors
from reportlab.lib.styles import getSampleStyleSheet, ParagraphStyle
from reportlab.platypus import (
    SimpleDocTemplate, Paragraph, Spacer, Table, TableStyle, PageBreak, HRFlowable
)

OUT = "/home/mike/.zeus/workspace/vmctl-tui/docs/vmctl-app-requirements.pdf"

styles = getSampleStyleSheet()
H1 = ParagraphStyle("H1", parent=styles["Heading1"], fontSize=18, spaceAfter=10, textColor=colors.HexColor("#1a2b4a"))
H2 = ParagraphStyle("H2", parent=styles["Heading2"], fontSize=13, spaceBefore=12, spaceAfter=6, textColor=colors.HexColor("#2c4a7c"))
H3 = ParagraphStyle("H3", parent=styles["Heading3"], fontSize=11, spaceBefore=8, spaceAfter=4, textColor=colors.HexColor("#3a5a8c"))
BODY = ParagraphStyle("BODY", parent=styles["BodyText"], fontSize=9.5, leading=13, spaceAfter=5)
SMALL = ParagraphStyle("SMALL", parent=styles["BodyText"], fontSize=8.5, leading=11, textColor=colors.HexColor("#555555"))
BULLET = ParagraphStyle("BULLET", parent=BODY, leftIndent=14, bulletIndent=4, spaceAfter=3)

def cell(text, bold=False):
    return Paragraph(f"<b>{text}</b>" if bold else text, SMALL)

def table(headers, rows, widths):
    data = [[cell(h, bold=True) for h in headers]] + [[cell(c) for c in r] for r in rows]
    t = Table(data, colWidths=widths, repeatRows=1)
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#2c4a7c")),
        ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
        ("GRID", (0, 0), (-1, -1), 0.4, colors.HexColor("#b0b8c8")),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("ROWBACKGROUNDS", (0, 1), (-1, -1), [colors.white, colors.HexColor("#eef2f8")]),
        ("LEFTPADDING", (0, 0), (-1, -1), 5),
        ("RIGHTPADDING", (0, 0), (-1, -1), 5),
        ("TOPPADDING", (0, 0), (-1, -1), 3),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 3),
    ]))
    return t

def bullets(items):
    return [Paragraph(f"• {i}", BULLET) for i in items]

story = []

# ---- Title ----
story.append(Paragraph("vmctl — Application Requirements", H1))
story.append(Paragraph("A vm-bhyve management app: terminal UI (TUI) and desktop GUI, with a built-in ease-of-use assistant.", BODY))
story.append(Paragraph("Version 0.1 · Draft for design review · Target platform: FreeBSD (vm-bhyve v1.7.5), portable to Linux/macOS backends.", SMALL))
story.append(HRFlowable(width="100%", thickness=1, color=colors.HexColor("#2c4a7c"), spaceBefore=6, spaceAfter=10))

# ---- 1. Overview ----
story.append(Paragraph("1. Overview &amp; Goals", H2))
story.append(Paragraph(
    "vmctl is a first-class management front-end for <b>vm-bhyve</b>, designed to feel as approachable as VirtualBox "
    "while exposing the full power of bhyve. It ships in two front-ends that share one core: a <b>TUI</b> (ratatui, runs "
    "in any terminal, including over SSH) and a <b>Desktop GUI</b> (native windowed app). Both are driven by a shared "
    "Rust core and a plain-English <b>assistant</b> that turns intent into safe vm(8) operations.", BODY))
story.append(Paragraph("Design goals, in priority order:", BODY))
story.extend(bullets([
    "<b>Powerful</b> — cover the full vm-bhyve lifecycle: create, install, start/stop, snapshot/clone, networking, storage, console.",
    "<b>Easy to use</b> — VirtualBox-style flows: a creation wizard, one-click actions, sensible defaults, no config-file editing required for common tasks.",
    "<b>Safe</b> — destructive actions (destroy, discard, rollback, poweroff) are confirmed and, where possible, reversible via snapshot.",
    "<b>Honest</b> — surface real system state and real errors; never pretend an action succeeded.",
    "<b>Portable core</b> — the vm-bhyve backend is one implementation behind a trait, so a future VirtualBox/QEMU backend can slot in.",
]))

# ---- 2. Personas ----
story.append(Paragraph("2. Users &amp; Use Cases", H2))
story.append(table(
    ["Persona", "Need", "Primary surface"],
    [
        ["Desktop user", "Run VMs without learning bhyve; create a VM from an ISO in a few clicks.", "Desktop GUI + assistant"],
        ["Sysadmin / power user", "Manage many VMs over SSH; script and automate; full control.", "TUI + CLI fast-path"],
        ["Developer", "Spin up disposable test VMs, snapshot/rollback, iterate quickly.", "TUI + Desktop"],
        ["Newcomer", "Doesn't know vm(8) syntax; asks in plain English.", "Assistant (both)"],
    ],
    [1.3*inch, 3.6*inch, 1.6*inch],
))

# ---- 3. Architecture ----
story.append(Paragraph("3. Architecture (shared core)", H2))
story.append(Paragraph(
    "One core crate (<b>vmctl-core</b>) owns all logic; the TUI and Desktop are thin views over it. This guarantees the "
    "two front-ends never diverge in behavior.", BODY))
story.append(table(
    ["Layer", "Responsibility", "Notes"],
    [
        ["vmctl-core", "Typed vm(8) wrapper, VM model, actions, state machine, assistant engine.", "Already scaffolded: vmbhyve.rs, actions.rs"],
        ["Backend trait", "Abstract VM ops (list/create/start/stop/…). vm-bhyve is the first impl.", "Future: VirtualBox, QEMU, cloud"],
        ["Assistant engine", "Plain-English → intent → validated action plan → confirm → execute.", "Shared by TUI + Desktop"],
        ["TUI (ratatui)", "Terminal panes: list, wizard, config, console, logs.", "ca0e750: list + actions done"],
        ["Desktop GUI", "Windowed app: same views as native widgets + drag/drop + system tray.", "e.g. egui / Tauri / Iced"],
    ],
    [1.3*inch, 3.6*inch, 1.6*inch],
))

story.append(PageBreak())

# ---- 4. Functional requirements ----
story.append(Paragraph("4. Functional Requirements", H2))

story.append(Paragraph("4.1 VM Lifecycle &amp; Inventory", H3))
story.append(table(
    ["ID", "Requirement", "Priority"],
    [
        ["F-101", "List all VMs with name, state, vCPUs, memory, datastore, and console info (parses multi-word states like 'Locked (user)').", "Must"],
        ["F-102", "Create a VM via wizard: template → datastore → size → memory → vCPUs → image/ISO → name (maps to `vm create`).", "Must"],
        ["F-103", "Install from ISO (`vm install <name> <iso>`) with a guided flow and progress.", "Must"],
        ["F-104", "Start / stop / restart / suspend / reset / poweroff single or multiple VMs.", "Must"],
        ["F-105", "Start-all / stop-all / suspend-all bulk operations.", "Should"],
        ["F-106", "Show detailed VM info (`vm info`) in a structured panel.", "Must"],
        ["F-107", "Rename, clone, and discard/delete VMs (with confirmation).", "Should"],
        ["F-108", "Attach to VM console (`vm console`) — surfaced as a safe action; in TUI offered as a shell hand-off, in Desktop as an embedded terminal.", "Must"],
    ],
    [0.6*inch, 5.4*inch, 0.7*inch],
))

story.append(Paragraph("4.2 Snapshots &amp; Images", H3))
story.append(table(
    ["ID", "Requirement", "Priority"],
    [
        ["F-201", "Create, list, rollback, and delete snapshots (`vm snapshot/rollback`).", "Should"],
        ["F-202", "Clone from a snapshot (`vm clone name@snap newname`).", "Should"],
        ["F-203", "Manage images: list, create, provision, destroy (`vm image …`).", "Could"],
        ["F-204", "Download/list available ISOs and images (`vm iso`, `vm img`).", "Could"],
    ],
    [0.6*inch, 5.4*inch, 0.7*inch],
))

story.append(Paragraph("4.3 Networking &amp; Storage", H3))
story.append(table(
    ["ID", "Requirement", "Priority"],
    [
        ["F-301", "Manage virtual switches: list, create (standard/standard-nat/private), edit, destroy (`vm switch …`).", "Should"],
        ["F-302", "Manage datastores: list, add, remove (`vm datastore …`).", "Should"],
        ["F-303", "Add devices to a VM: NIC, disk, custom (`vm add -t/-s`).", "Should"],
        ["F-304", "Edit raw VM config in a safe editor with validation (`vm configure/edit`).", "Could"],
    ],
    [0.6*inch, 5.4*inch, 0.7*inch],
))

story.append(Paragraph("4.4 Assistant (ease-of-use layer)", H3))
story.append(table(
    ["ID", "Requirement", "Priority"],
    [
        ["F-401", "Accept plain-English intent (e.g. 'make a 4GB Ubuntu VM with 2 CPUs') and propose a concrete action plan.", "Must"],
        ["F-402", "Show the plan (the exact vm(8) operations) and require confirmation before executing anything.", "Must"],
        ["F-403", "Suggest a sensible default action for the selected VM (stopped → Start, running → Console/Stop).", "Must"],
        ["F-404", "Explain errors and system state in plain language (e.g. non-root permission, missing ISO).", "Should"],
        ["F-405", "Answer 'how do I …?' questions with the exact steps/commands.", "Should"],
    ],
    [0.6*inch, 5.4*inch, 0.7*inch],
))

story.append(PageBreak())

# ---- 5. Non-functional ----
story.append(Paragraph("5. Non-Functional Requirements", H2))
story.append(table(
    ["ID", "Category", "Requirement"],
    [
        ["N-101", "Safety", "Destructive ops require explicit confirmation; never auto-run on selection. Assistant always shows the plan first."],
        ["N-102", "Safety", "Privilege handling: detect non-root and surface a clear, actionable error rather than a crash."],
        ["N-103", "Reliability", "Terminal state is always restored on exit (no stuck raw mode). GUI never blocks the main thread on vm(8) calls."],
        ["N-104", "Performance", "VM list refresh &lt; 200ms for typical fleets; async/background execution of long ops (install, start)."],
        ["N-105", "Portability", "Core is OS-agnostic behind the backend trait; TUI works over SSH; Desktop targets FreeBSD/Linux/macOS."],
        ["N-106", "Testability", "All parsing and action-mapping logic unit-tested; backend trait mockable for UI tests."],
        ["N-107", "Accessibility", "TUI: full keyboard control, no mouse required. Desktop: keyboard shortcuts, screen-reader-friendly labels."],
        ["N-108", "Honesty", "UI reflects real vm(8) exit status; failed actions show the real stderr, never a false success."],
    ],
    [0.6*inch, 1.0*inch, 5.1*inch],
))

# ---- 6. UX ----
story.append(Paragraph("6. UX Requirements (VirtualBox-style ease of use)", H2))
story.extend(bullets([
    "<b>Creation wizard</b> — a step-through flow (Template → Resources → Storage → Network → Summary → Create) with recommended defaults pre-filled.",
    "<b>One-click actions</b> — a prominent action bar / toolbar; the most likely action is highlighted.",
    "<b>At-a-glance state</b> — color-coded VM states (running/stopped/locked/suspended) in both views.",
    "<b>Contextual help</b> — the assistant is always one keystroke/click away and knows the current selection.",
    "<b>Progress &amp; feedback</b> — long operations (install, clone) show progress; all actions give clear success/failure feedback.",
    "<b>Undo where possible</b> — snapshot-before-destructive-op as an opt-in safety net.",
]))

# ---- 7. Scope ----
story.append(Paragraph("7. Scope &amp; Milestones", H2))
story.append(table(
    ["Milestone", "Deliverable", "Status"],
    [
        ["M0 — Core foundation", "Typed vm(8) wrapper + list parser + action layer.", "Done (0c18a73, ca0e750)"],
        ["M1 — TUI MVP", "VM list pane, action bar, create wizard, console hand-off.", "In progress"],
        ["M2 — Assistant", "Plain-English → plan → confirm → execute, in TUI.", "Planned"],
        ["M3 — Desktop GUI", "Windowed app over the same core: list, wizard, actions, tray.", "Planned"],
        ["M4 — Snapshots/net/storage", "Snapshot/clone, switch &amp; datastore management in both views.", "Planned"],
    ],
    [1.5*inch, 3.6*inch, 1.6*inch],
))

story.append(Spacer(1, 10))
story.append(Paragraph(
    "Out of scope for v1: live migration, clustering/federation, multi-host orchestration, and non-bhyve backends "
    "(the backend trait is designed to allow them later).", SMALL))

doc = SimpleDocTemplate(OUT, pagesize=letter,
                        leftMargin=0.75*inch, rightMargin=0.75*inch,
                        topMargin=0.7*inch, bottomMargin=0.7*inch,
                        title="vmctl — Application Requirements (TUI + Desktop)")
doc.build(story)
print("WROTE", OUT)
