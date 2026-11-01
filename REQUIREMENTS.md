# requirements

product requirements for mudita. it is never okay to regress on these in a
release. each requirement has a stable id so tests and the roadmap can refer
to it.

DRAFT: written from the initial design discussion and awaiting review by the
product owner.

## functional

- **R1 direct transfer.** file contents travel directly from a device that
  has the file to a device that wants it, over veilid. file contents are
  never stored in the dht or on any server.
- **R2 share by link.** sharing a file produces a share token and a link
  that carries it. anyone holding either can download the file for as long as
  a device sharing it stays running.
- **R3 verified pieces.** every piece of a download is checked against the
  hash published by the sharer before it is accepted. a download either
  completes with exactly the shared bytes or fails; it never yields a
  silently corrupted file.
- **R4 command line.** a file can be shared and downloaded from a terminal
  with the `dana` command.
- **R5 app.** a file can be shared and downloaded from the app, including in
  a normal desktop browser with no install.
- **R6 interoperability.** the command line and the app speak the same
  protocol: a file shared from either can be downloaded by either.
- **R7 large files.** sharing a file does not require loading it into memory
  whole, and the protocol sets no practical ceiling on file size.
- **R8 route recovery.** a share link keeps working when the sharer's veilid
  route changes while it is running.

## non-functional

- **R9 private by default.** peers reach each other only through veilid
  private routes, so neither side learns the other's ip address. the web app
  runs veilid in the browser (wasm), so no gateway server sees file data or
  share tokens.
- **R10 deterministic builds.** toolchains are pinned (mise) and the veilid
  dependency is pinned to one release across the app and the command line.
- **R11 minimal dependencies.** prefer the veilid sdk and the standard
  libraries over adding third-party packages.
