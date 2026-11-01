# contributing

## toolchain

flutter, rust, and uv are pinned in `mise.toml`; run `mise install` once.
every make target invokes its tools through `mise exec`, so builds do not
depend on whatever is on your path.

veilid is pinned to one release, by exact version, in `core/Cargo.toml`. the
web build of the core also needs `wasm-bindgen` at the release veilid uses;
`scripts/build_core_wasm.sh` installs it.

## common commands

```
make                 # build dana and the production web app
make build-dana      # dana only, into target/release/dana
make build-bindu-web # bindu for veilid.tech/mudita (WEB_BASE_HREF overrides /mudita/)
make build-bindu     # bindu for linux, into build/bindu
make run-bindu       # run bindu on linux
make build-mudita    # the desktop app, for linux, into build/mudita
make run-mudita      # run the desktop app
make wasm            # rebuild the core for the browser under web/wasm/
make fmt             # format dart, rust, and python
make precommit       # format check, static analysis, unit tests
make test-e2e        # live-network transfers between dana and the web app
make screenshots     # redraw the pictures in README.md, into docs/screenshots
```

## workflow

- product requirements live in `REQUIREMENTS.md` and are never regressed
  without the product owner's approval.
- update `DESIGN.md` after architectural or protocol changes and `README.md`
  after user-visible ones.
- for a bug: write a failing test first, then fix it.
- run `make precommit` before handing work over.

## one implementation

the protocol, the sharing and downloading, the library and the logging are
written once, in rust, in `core/`. everything else is a way of running it or
a face on it:

- `cli/` runs the core on a desktop or server (`dana`).
- `wasm/` runs the core in a web page.
- `lib/` is the flutter app. it has no protocol code. it asks the core for
  everything through the control methods in `DESIGN.md`, over a socket on
  desktop and by calling the wasm module on web.

so a change to what mudita does is made in `core/`, in this order:

1. `DESIGN.md`.
2. `testdata/protocol_vectors.json`, which the core's unit tests read. its
   hashes use a toy hash described in the file, so the tests need no
   network.
3. the core, until `make test` passes.
4. the app, if what it shows or asks for changes.
5. `make test-e2e`, which is the only check that the builds of the core
   really work together over the network.

a behaviour is never added to the app alone. if the app needs something the
core does not offer, the core gets a control method for it.

## names

- mudita is the project, and its full desktop app: folder sharing, browsing
  and search. `lib/desktop/`, for linux, built by `build-mudita`.
- bindu is the mini app: `lib/main.dart` with `lib/ui/`, on web and linux,
  built by `build-bindu-web` and `build-bindu`. it stays small: one file is
  sent at a time, and files are received one by one.
- dana is the command line and the server: `cli/`, built by `build-dana`.
- a library is one name over one or more folders, with a link of its own.

the project's own paths keep its name whichever program uses them:
`~/.config/mudita/`, `mudita.sock`, the `MUDITA_*` variables, `/mudita/` on
the web, and the core's `mudita::` log targets. the two linux apps are one
flutter project and one runner: the makefile gives each bundle its binary
name and, through `MUDITA_APP_ID`, its application id.

both apps are clients of the core. library features are built in the core
first, then shown in the window.

## tests

- `make test` is hermetic. rust tests cover everything in the core that is
  pure: the wire format, the merkle tree, the bookkeeping, the log lines.
  dart tests drive the app against fakes of the core (`test/fakes/`): an
  in-memory one for the screens, and one on a real unix socket for the
  desktop transport. there is no fake veilid network, so the core's
  networked paths are covered only by the e2e tests.
- `make test-e2e` (python + pytest, `test/e2e/`) starts real veilid nodes:
  `dana` processes, the core alone in a bare web page, and the web app in
  chrome via playwright. the web app is driven through `window.muditaTest`,
  a hook compiled in only by `make build-bindu-web-e2e`, because flutter web
  paints to a canvas and has no dom to click through. pages are served
  as they are deployed, over plain http from a name that is not localhost
  (browsers treat localhost as secure), with a few tests on a secure origin
  for the other download store. `test_glue.py` tests the browser storage
  code alone, with no node. the
  tests share the live network with everyone else, so none may assume it
  is alone in the lobby: a test names its libraries and files after its
  own run and looks for those. a test server is started with
  `MUDITA_TEST_PROBE` set, which turns on the `probe` control method
  (`link`, `root`): it asks the library a link leads to for the first piece
  of a file and says what that answered, which is how a test shows that a
  library's route serves its own files and not those of another library on
  the same server. users have no such method.
- `make screenshots` (`test/screenshots/`) is no test: it draws the
  desktop app over the fake server, staged with a made-up library,
  and writes what it shows to `docs/screenshots/`. run it after changing
  how the window looks. it needs `fc-match`, to find the desktop's
  monospace font; the times in the pictures are those of the run.

## style

- ascii only. docs, comments, and user-facing text are lowercase.
- comments say why. each function gets a one-line description at most.
- named constants at the top of the file instead of inline numbers.
- keep dependencies to the veilid sdk and the standard libraries where
  possible; justify each addition where it is declared.
