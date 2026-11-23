Edit by subtraction: resolve a problem by deleting code; a tactical patch over a symptom is not accepted. One implementation per thing, never two alive.

The history of a codebase should not be encoded in or inferrable from the codebase itself. Systems at rest should look as if they were designed this way from the start. Exceptions exist, including git history and changelogs. Implications include immediate refactors to simplify and remove old patterns, and immediate sweeps and cleanups of code and documentation included with each modification.

Delete code comments; keep only a why the code cannot show.

## Boundaries

Keep this project independent. Reference other projects only as declared, versioned
dependencies, exposing names and versions rather than internals. Give shared services
neutral project-owned names. Exclude deployment-specific paths, addresses, service
or queue names, credentials, camera frames and private renders. Before landing,
inspect the diff for undeclared project references and deployment details.

## Teaching with architecture objects

In the headset a hub can answer a question about a model architecture by showing it. [vr/ARCHITECTURE.md](vr/ARCHITECTURE.md) has what the object draws, the description format, the node names and the live data; [vr/OBJECTS.md](vr/OBJECTS.md), "In this overlay", has what a reply's display item does with an object. The display item's fields are `object`, `data`, `point`, `hold` and `clear`, as `vr/src/hub.rs` reads them; how a hub sends a reply is its own.

- Build: `nix-build vr/architecture --no-out-link` prints a directory holding the ready objects. For an architecture none of them covers, write a description into a directory of its own and build that with `--arg descriptions ./<directory>`. Models to be compared go in one description: each object is shown at one size, so two objects side by side share no scale.
- Show: a reply whose display item carries the `.glb` as its `object`; with `clear` it stands alone.
- Drive: a later reply carries `data` and no object, such as `map pick` for `reuse` or `weigh` for `budget`. An object left three minutes without data or a touch goes away.
- Point: `point` names a node, such as `module3`, `chunk1`, `bank` or `stream`, or `narrow.chunk1` in an object of several models. The puppet points at it while the reply is spoken.
- Step: `hold` at *i* + 1.5 stops the token in module *i*; `hold` false lets it run again.
- One idea a reply: say what to look at, point at it, change one thing. The object carries no words, so the first reply that shows one says what its parts are: a plate is stored once, a frame is computed each time a token passes.
