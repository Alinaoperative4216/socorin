# Security policy

Socorin runs with the Screen Recording permission, listens for global
shortcuts and writes to the clipboard, so a bug in it can matter more than
in an ordinary desktop app. Reports are welcome.

## Supported versions

Only the latest release is supported. Installed copies check
`https://socorin.com/version.json` once a day and offer the update; fixes
are not backported to older versions.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Use GitHub's
private vulnerability reporting instead: open the repository's **Security**
tab and choose **Report a vulnerability**. Include the version (shown in
Settings), the platform, the steps to reproduce and, if you have one, a
proof of concept.

You will get an acknowledgement within seven days. Once a fix is released
the report is credited in the release notes unless you prefer otherwise.

## What counts

Examples of what we treat as a vulnerability:

- Taking a screenshot or a recording without the user's action, for example
  a way around the *Allow command-line triggers* switch.
- Installing an update that is not signed with the project's updater key,
  or reading the manifest from anywhere but the configured endpoint.
- Writing outside the chosen screenshots folder other than through the
  system save dialog (the file-name prefix is sanitised, and the webviews
  cannot name a destination: the dialog runs on the Rust side).
- Running an `ffmpeg` other than the configured one or the one found on
  `PATH`, or making the configured path run anything but `ffmpeg`.
- A webview reaching a plugin command or a URL its capability
  (`src-tauri/capabilities/default.json`) does not list.
- Anything the webviews' Content Security Policy is meant to prevent.

The behaviour of the third-party programs the app calls (`screencapture`,
`ffmpeg`, the system clipboard) is out of scope; report those upstream.
