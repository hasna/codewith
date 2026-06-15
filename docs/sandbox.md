# Sandbox & Approvals

Codewith can run tool commands in a restricted sandbox so file and network
access match the current permission profile.

## Platforms

- macOS uses Seatbelt sandboxing.
- Linux uses Landlock and bubblewrap where available.
- Windows uses Windows sandboxing and can launch sandboxed children on a private
  desktop.

## Permissions

Use `/permissions` in the TUI to inspect or change what Codewith is allowed to
do. Permission profiles control whether Codewith may read files, write files,
use the network, or request elevated execution.

When a command needs access outside the current profile, Codewith asks for
approval instead of silently expanding permissions.

## Extra Read Roots

Use `/sandbox-add-read-dir` to let the sandbox read an additional absolute
directory:

```text
/sandbox-add-read-dir /absolute/path
```

Keep this list narrow. It is meant for project-adjacent context, not broad home
directory access.
