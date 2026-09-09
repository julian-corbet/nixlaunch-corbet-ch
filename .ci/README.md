Checks are selected through `.ci/ccid.toml` and run by the pinned shared ccid runner on a trusted worker.

The default selection is `native,headless`. Native Linux success does not certify a foreign architecture, a separately selected image or hardware gate, or publication. Use `list` to inspect available native Nix checks, and select existing results or affected checks before scheduling more work.

Additional coverage limits:

- Existing Crow performs real headless compositor behavior only; preserve safe private compositor socket proof and add native package checks separately.

Hosted Actions keeps the full portable workflow available for manual fallback.
