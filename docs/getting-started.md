# Getting started with Codewith

Codewith is a local terminal coding agent. Install it, authenticate, and run it
from the repository or project directory you want it to work in.

## Install

```shell
bun install -g @hasna/codewith
```

You can also install it with npm:

```shell
npm install -g @hasna/codewith
```

## Sign in

Run Codewith and select a login method:

```shell
codewith
```

You can also authenticate from the command line:

```shell
codewith login
codewith login --with-api-key
```

Codewith stores local state in `~/.codewith` unless `CODEWITH_HOME` is set.

## Start working

Run interactive Codewith in a project:

```shell
cd path/to/project
codewith
```

For one-off non-interactive work, use `exec`:

```shell
codewith exec "summarize the current changes"
```

For a code review pass, use `review`:

```shell
codewith review
```

Inside the TUI, type `/` to browse commands. Common starting points are
`/model`, `/profile`, `/config`, `/permissions`, `/review`, `/diff`, and
`/resume`.
