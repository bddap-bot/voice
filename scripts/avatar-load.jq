def attrs: map({key, value: .value.stringValue}) | from_entries;
def class: if test("iPhone|iPod|Android.*Mobile") then "phone" elif test("iPad|Android") then "tablet" elif test("CrKey|SMART-TV|Tizen|Web0S") then "tv" else "desktop" end;
def pct($p): .[(($p * length) | ceil) - 1];
(["class", "stage", "loads", "p50_ms", "p90_ms"] | @tsv),
([inputs | .resourceSpans[]? | select(.resource.attributes // [] | attrs | .["service.name"] == "page") | .scopeSpans[].spans[] | select(.status.code != 2)]
| (map(select(.name == "avatar-load")) | map({key: .spanId, value: (.attributes | attrs | .["user_agent.original"] | class)}) | from_entries) as $class
| map(select($class[.parentSpanId // ""]) | {class: $class[.parentSpanId], stage: .name, ms: (((.endTimeUnixNano | tonumber) - (.startTimeUnixNano | tonumber)) / 1e6)})
| group_by([.class, .stage])[] | (map(.ms) | sort) as $ms
| [.[0].class, .[0].stage, ($ms | length), ($ms | pct(0.5) | round), ($ms | pct(0.9) | round)]
| @tsv)
