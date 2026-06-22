import SwiftUI

struct TaskResultView: View {
    var showDiffPanel: Bool = false

    var body: some View {
        VStack(spacing: 0) {
            // Top bar
            HStack(spacing: 8) {
                Text("Add abstract OAuth preparation").font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Image(systemName: "ellipsis").font(.system(size: 12)).foregroundStyle(Theme.textTertiary)
                Spacer()
                topItem("arrow.up.right", "Open")
                topItem("arrow.triangle.branch", "PR 5")
                if showDiffPanel { topItem("checkmark.circle", "Review") }
                Image(systemName: "square.righthalf.filled").font(.system(size: 13)).foregroundStyle(Theme.textTertiary).padding(.leading, 6)
            }
            .padding(.horizontal, 16).frame(height: 40)
            Rectangle().fill(Theme.separator).frame(height: 1)

            HStack(spacing: 0) {
                ScrollColumn(alignment: .leading, spacing: 0) {
                    HStack { Spacer()
                        Text("add abstract oauth preparation").font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
                            .padding(.horizontal, 10).padding(.vertical, 5)
                            .background(RoundedRectangle(cornerRadius: 8).fill(Theme.fieldFill))
                    }
                    .padding(.bottom, 10)

                    sectionHeader("Notes")
                    bullet("black . failed with a parsing error in ", code: "cli/commands/auth.py", suffix: " and could not reformat the code")
                    bullet("pytest could not run because the ", code: "httpx", suffix: " package is missing in this environment")

                    sectionHeader("Summary")
                    bullet("Listed the new OAuth preparation guide in the feature bullets of the README")
                    bullet("Documented the presence of the OAuth preparation guide in the implementation folder structure")
                    bullet("Added a new file describing generic steps to prepare OAuth for any integration")

                    sectionHeader("Testing")
                    testRow(false, "black .", "(failed to parse cli/commands/auth.py)")
                    testRow(true, "ruff check .", "(with deprecation warnings)")
                    testRow(true, "mypy .", "(no output indicates success)")
                    testRow(false, "pytest", "(failed to import httpx)")

                    editedFilesCard().padding(.top, 10)

                    applyBanner().padding(.top, 10)

                    Composer(placeholder: "Create a new local task that references this cloud task")
                        .padding(.top, 8)
                    HStack(spacing: 5) {
                        Image(systemName: "desktopcomputer").font(.system(size: 10))
                        Text("Local").font(.system(size: 11.5))
                        Image(systemName: "chevron.down").font(.system(size: 8))
                    }
                    .foregroundStyle(Theme.textSecondary).padding(.top, 8)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 28).padding(.top, 16)
                .frame(maxWidth: .infinity)

                if showDiffPanel {
                    Rectangle().fill(Theme.separator).frame(width: 1)
                    DiffPanel().frame(width: 300)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.canvas)
    }

    private func topItem(_ icon: String, _ label: String) -> some View {
        HStack(spacing: 4) { Image(systemName: icon).font(.system(size: 10)); Text(label).font(.system(size: 11.5)) }
            .foregroundStyle(Theme.textSecondary).padding(.leading, 14)
    }
    private func sectionHeader(_ t: String) -> some View {
        Text(t).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.textPrimary).padding(.top, 9).padding(.bottom, 4)
    }
    private func bullet(_ pre: String, code: String? = nil, suffix: String = "") -> some View {
        var s = AttributedString(pre)
        s.font = .system(size: 12.5)
        s.foregroundColor = Theme.textPrimary
        if let code {
            var c = AttributedString(code)
            c.font = .system(size: 11, design: .monospaced)
            c.foregroundColor = Color(hex: 0x1F2328)
            c.backgroundColor = Color(hex: 0xF3F4F6)
            s.append(c)
            var suf = AttributedString(suffix)
            suf.font = .system(size: 12.5)
            suf.foregroundColor = Theme.textPrimary
            s.append(suf)
        }
        return HStack(alignment: .top, spacing: 8) {
            Circle().fill(Theme.textSecondary).frame(width: 3.5, height: 3.5).padding(.top, 6).padding(.leading, 2)
            Text(s).fixedSize(horizontal: false, vertical: true)
            Spacer()
        }
        .padding(.bottom, 4)
    }
    private func testRow(_ ok: Bool, _ cmd: String, _ note: String) -> some View {
        HStack(spacing: 8) {
            RoundedRectangle(cornerRadius: 4, style: .continuous)
                .fill(ok ? Color(hex: 0x34C759) : Color(hex: 0xFF3B30))
                .frame(width: 15, height: 15)
                .overlay(Image(systemName: ok ? "checkmark" : "xmark").font(.system(size: 9, weight: .bold)).foregroundStyle(.white))
            Text(cmd).font(.system(size: 11.5, design: .monospaced)).foregroundStyle(Theme.textPrimary)
            Text(note).font(.system(size: 11.5)).foregroundStyle(Theme.textSecondary)
            Spacer()
        }
        .padding(.bottom, 4)
    }
    private func editedFilesCard() -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "square.stack.3d.up").font(.system(size: 12)).foregroundStyle(Theme.textSecondary)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Edited 3 files").font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.textPrimary)
                    Text("+46 -1").font(.system(size: 10.5)).foregroundStyle(Theme.textSecondary)
                }
                Spacer()
                HStack(spacing: 4) { Image(systemName: "arrow.uturn.backward").font(.system(size: 10)); Text("Undo").font(.system(size: 11.5)) }.foregroundStyle(Theme.textSecondary)
                Text("Review").font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.textPrimary).padding(.leading, 8)
            }
            .padding(12)
            Rectangle().fill(Theme.separator).frame(height: 1)
            fileRow("README.md", "+1", "-0")
            fileRow("implementation/folder_structure.md", "+2", "-1")
            fileRow("implementation/oauth_prep.md", "+43", "-0")
        }
        .background(RoundedRectangle(cornerRadius: 10).fill(Color.white).overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.cardStroke, lineWidth: 1)))
    }
    private func fileRow(_ name: String, _ add: String, _ del: String) -> some View {
        HStack {
            Text(name).font(.system(size: 11.5)).foregroundStyle(Theme.textPrimary)
            Spacer()
            Text(add).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.success)
            Text(del).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.textTertiary)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
    }
    private func applyBanner() -> some View {
        HStack(spacing: 10) {
            Image(systemName: "arrow.down.doc").font(.system(size: 14)).foregroundStyle(Theme.textSecondary)
            VStack(alignment: .leading, spacing: 2) {
                Text("Apply changes and continue locally?").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Text("This task was made in hasna/scaffold-api so may not apply cleanly.").font(.system(size: 11)).foregroundStyle(Theme.warning)
            }
            Spacer()
            Text("Apply").font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.textPrimary)
                .padding(.horizontal, 14).frame(height: 28)
                .background(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.cardStroke, lineWidth: 1))
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 10).fill(Color(hex: 0xFCFAF5)).overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.cardStroke, lineWidth: 1)))
    }
}

/// The "Cloud changes" diff side panel (reference screenshot 10).
struct DiffPanel: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Cloud changes").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.textPrimary)
                Text("+46 -1").font(.system(size: 10.5, design: .monospaced)).foregroundStyle(Theme.textSecondary)
                Spacer()
            }
            .padding(.horizontal, 12).frame(height: 34)
            Rectangle().fill(Theme.separator).frame(height: 1)
            ScrollColumn(alignment: .leading, spacing: 0) {
                diffFileHeader("implementation/folder_structure.md", "+2 -1")
                hunkHeader("@@ -11,6 +11,6 @@")
                diffLine("11", "Instructions for Claude Code", .plain)
                diffLine("12", "Shared utilities for logging and error h…", .plain)
                hunkHeader("@@ -44,2 +44,4 @@")
                diffLine("44", "CLI command implementations", .plain)
                diffLine("45", "flip.py deployment configuration", .add)
                diffLine("46", "Python project settings", .add)
                hunkHeader("@@ -101,1 +101,3 @@")
                diffLine("101", "Guide for setting up new API integ…", .del)
                diffLine("102", "Guide for setting up new API integr…", .add)
                diffLine("103", "Abstract guide for preparing OAuth…", .add)
                diffFileHeader("implementation/oauth_prep.md", "+43 -0")
                hunkHeader("@@ -0,0 +1,43 @@")
                diffLine("1", "# OAuth Preparation Guide", .add)
                diffLine("2", "", .add)
                diffLine("3", "This document outlines general steps …", .add)
                Spacer(minLength: 0)
            }
        }
        .background(Color(hex: 0xFAFAFA))
    }
    private func diffFileHeader(_ name: String, _ stat: String) -> some View {
        HStack {
            Image(systemName: "doc.text").font(.system(size: 10)).foregroundStyle(Theme.textTertiary)
            Text(name).font(.system(size: 10.5, design: .monospaced)).foregroundStyle(Theme.textPrimary).lineLimit(1)
            Spacer()
            Text(stat).font(.system(size: 9.5, design: .monospaced)).foregroundStyle(Theme.textSecondary)
        }
        .padding(.horizontal, 10).padding(.vertical, 7)
        .background(Color(hex: 0xF0F0F2))
    }
    enum K { case plain, add, del }
    private func hunkHeader(_ text: String) -> some View {
        HStack(spacing: 0) {
            Text(text).font(.system(size: 9, design: .monospaced)).foregroundStyle(Color(hex: 0x6E8BB5))
            Spacer()
        }
        .padding(.horizontal, 8).padding(.vertical, 2)
        .background(Color(hex: 0xEAF0FB))
    }
    private func diffLine(_ num: String, _ text: String, _ k: K) -> some View {
        HStack(spacing: 0) {
            Text(num).font(.system(size: 8.5, design: .monospaced)).foregroundStyle(Theme.textTertiary)
                .frame(width: 16, alignment: .trailing)
                .padding(.trailing, 4)
            Text(k == .add ? "+" : (k == .del ? "−" : " "))
                .font(.system(size: 9.5, design: .monospaced))
                .foregroundStyle(k == .add ? Theme.success : (k == .del ? Theme.danger : Theme.textTertiary))
                .frame(width: 7)
            Text(text).font(.system(size: 9.5, design: .monospaced)).foregroundStyle(Theme.textPrimary).lineLimit(1)
            Spacer(minLength: 0)
        }
        .padding(.trailing, 6).padding(.vertical, 1.0)
        .background(k == .add ? Color(hex: 0xE6FFEC) : (k == .del ? Color(hex: 0xFFEBE9) : Color.clear))
        .overlay(alignment: .leading) { Rectangle().fill(Color(hex: 0xEFEFEF)).frame(width: 1).padding(.leading, 20) }
    }
}
