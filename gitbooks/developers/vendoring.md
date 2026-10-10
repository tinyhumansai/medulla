# Vendored dependencies

| Submodule | Upstream | Purpose |
| --- | --- | --- |
| `vendor/openhuman` | `tinyhumansai/openhuman` | Native agent runtime through the `openhuman-embed` facade |
| `vendor/tinyflows` | `tinyhumansai/tinyflows` | Independent Medulla DAG engine |
| `vendor/tinyhumans-sdk` | `tinyhumansai/sdk` | Backend HTTP client |

Run `bash scripts/init-submodules.sh` to initialize all recursive sources.
HTTPS remotes keep CI builds usable without deploy keys. The vendored trees
are excluded from workspace members and from Medulla's coverage denominator.

The SDK depends directly on `vendor/openhuman/crates/openhuman-embed`, with
`default-features = false` and `skills, storage-sqlite`. The root manifest
carries OpenHuman's patch tables because Cargo ignores dependency workspaces'
patches. Its tool, inference, and storage packages therefore resolve to the
same vendored paths used by OpenHuman. Medulla consumes `Tool` and `ToolResult`
through embed; it does not construct a second tool-trait graph.

Medulla's flow engine retains its independent `vendor/tinyflows` pin. Unifying
that pin with OpenHuman's workflow engine is a separate change. The backend
HTTP SDK also remains independent of the agent runtime.

To update a pin, fetch its remote and check out the intended commit inside the
submodule, initialize its recursive sources, run `make ci`, and commit the
updated gitlink. Do not edit a vendored dependency in place: submit its change
to that repository and pin the resulting commit. Verify resolution with
`cargo tree -i openhuman-embed`, `cargo tree -i tinyflows`, and `cargo tree -d`.
