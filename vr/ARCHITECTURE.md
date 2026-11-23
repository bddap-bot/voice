# Architecture objects

`vr/architecture/generate.py` turns a description of one or more model architectures into an [object](OBJECTS.md): its parts are named nodes, and its program moves them by time, by the user's pointer and by live data.

```sh
nix-build vr/architecture --no-out-link                                # the ready descriptions, each as <store path>/<file name>.glb
nix-build vr/architecture --arg descriptions ./mine --no-out-link      # every *.json of another directory, given as a path
```

Without nix: `python3 vr/architecture/generate.py OUT description.json...`, with numpy, `rustc` for `wasm32-unknown-unknown`, and `PYTHONPATH` at a checkout of the wunderkammer-objects rev `vr/objects.nix` pins, whose glTF writer and program prelude it uses.

## What it draws

Every model is drawn in the same parts, so two of them compare part by part.

- A **stream**, the residual stream, is a grey pole; computation runs up it. Its thickness and the side of every square beside it follow the model's width.
- A **module** (a layer or block) is a hollow square frame beside the stream, joined to it by two rungs: the lower reads the stream, the upper adds to it. The stream is as many frames long as modules are computed.
- A **chunk** of stored weights is a solid square plate in the **bank**, off to the side.
- A **tie** runs from each frame to the plate it uses, and the frame wears that plate's colour: a plate with two ties is stored once and computed twice. Colours repeat after eight chunks.
- A module that picks its chunk per token wears no colour and has a faint tie to every chunk.
- A **token** is the bright bead on the stream. At each module a copy of the module's chunk leaves the bank and sits in the frame while a small bead goes in by the lower rung and comes back by the upper; from then on a pip of that chunk's colour circles the token, so at the top it carries what every module added.
- A model of two streams has a pole each side of its modules, `stream1` the one away from the bank; each rung is as thick as the module's weight for reading or writing that stream.

## Description

A JSON object `{"models": [...]}`; the models stand side by side, left to right. Sizes are relative to the widest model of the description and each object is shown at one size, so models to be compared go in one description. Each model:

| field | |
|---|---|
| `width` | the stream's width, a positive number, at least 1/32 of the widest model's |
| `modules` | how many modules are computed, in stream order; at most 256 |
| `chunks` | how many chunks of weights are stored, at most 256; default `modules` |
| `maps` | one or more maps from module to chunk, the first in use until live data names another; default `["own"]` when `chunks` equals `modules`, else `["mod"]` |
| `streams` | 1 (default) or 2 |
| `mix` | for two streams, and required then: per module `[read 1, read 2, write 1, write 2]`, each 0 to 1 |
| `name` | a lower-case letter, then lower-case letters, digits and `_`; none of the command words below, `all` or `off`; required and distinct when there are several models |

A map is one of:

| map | module *i* uses |
|---|---|
| `"own"` | chunk *i*; needs as many chunks as modules |
| `"mod"` | chunk *i* mod `chunks` |
| `{"random": seed}` | a chunk drawn once per module by Python's `random.Random(seed).randrange(chunks)`; a chunk may go unused |
| `{"table": [...]}` | the chunk listed for it |
| `{"loop": {"ends": e, "times": t}}` | the first and last *e* modules their own chunk, the chunks between run through in order *t* times; each pass is bracketed beside the stream |
| `"pick"` | one chunk per token; the choice shown is made up, fixed for a token and a module |

A map is named by its rule (`mod`, `random`, …) unless its object carries a name, as in `{"random": 6, "name": "scattered"}`; names follow the rule for model names and are distinct within a model.

```json
{ "models": [ { "width": 256, "modules": 8, "chunks": 4, "maps": ["mod", { "random": 6 }, "pick"] } ] }
```

The generator refuses a description that does not hold together, naming the fault, and one whose object would pass what a host draws (4096 nodes, 200,000 triangles): a `pick` map costs a tie and a pip for every module and chunk.

## Nodes

With one model the names are bare; with several each is prefixed by its model's name and a dot (`narrow.chunk2`). Modules, chunks and passes count from 0.

| node | |
|---|---|
| `stream`, or `stream1` and `stream2` | a stream |
| `token`, or `token1` and `token2` | the token on it |
| `module<i>` | module *i*, the lowest being 0 |
| `bank` | all the stored chunks |
| `chunk<c>` | chunk *c*'s plate |
| `pass<p>` | the bracket of pass *p* of a loop that is the first map; of another, `<map>.pass<p>` |
| `base` | the slab the model stands on |
| `model` for a lone model, else the model's name | the whole model |

## Time

`duration` is two seconds more than the deepest model has modules. The token enters in the first second and module *i* computes from *i* + 1 to *i* + 2 s, so held at *i* + 1.5 the token is level with module *i* and its chunk sits in its frame; in the last second it rises to the top, where a shallower model's token waits. Each further `duration` is the next token, which changes only what a `pick` map takes. Sliding the pointer up through the object carries the token up with it.

## Pointer

A pointer at a plate, or at the frame of a module with a fixed chunk, lifts that chunk and every frame that uses it out of the object's front and draws the rest back, for as long as it stays.

## Live data

Words separated by spaces or punctuation, up to 1024 bytes. A command takes the words after it that are its own, changes only what it names, and holds until changed again; one that names nothing the object has changes nothing. Frames, ties and columns ease into each change.

| command | |
|---|---|
| `map <name>` | every model that has a map of that name switches to it |
| `shared <c>` | in every model, lifts chunk *c* and the frames that use it, as the pointer does; `chunk<c>` names it too |
| `shared` | lifts each chunk in turn, about 1.8 s each |
| `shared off` | |
| `token <n>` | runs token *n* where time alone would run token 0, for a `pick` map held at a time |
| `weigh` | restacks each model into two columns on its base: where the frames were, hollow cells, what is computed for one token; where the bank was, solid cells, what is stored. A cell keeps its square's area and every cell is equally wide, so a column is as tall as the sum of its widths squared, and columns compare across the models of the object. `module<i>` and `chunk<c>` name the cells |
| `weigh off` | |
| `show <model>...` | keeps the named models, side by side in the middle, and puts the rest away |
| `show all` | |

## Ready descriptions

`vr/architecture/ready/`:

| file | models | live data to try |
|---|---|---|
| `transformer` | six modules on one stream, each with its own chunk | |
| `reuse` | eight modules on four chunks | `map mod`, `map random`, `map pick`, `shared` |
| `loop` | eight modules on five chunks: unshared ends, the middle three run twice | `shared 2` |
| `streams` | `one`: six modules on one stream; `two`: the same on two streams with mixing | `show two` |
| `budget` | `reuse`: 8 modules at width 256 on 4 chunks; `shallow`: 4 modules at 256; `narrow`: 8 at 184, storing 3% more than `shallow`; `full`: 8 at 256 | `weigh`, `show reuse narrow` |
