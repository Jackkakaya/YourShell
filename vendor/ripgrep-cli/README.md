# ripgrep CLI

This is the `crates/core` command frontend from ripgrep 15.1.0:
https://github.com/BurntSushi/ripgrep/releases/tag/15.1.0

The CLI parser, high-level argument conversion, search orchestration, output,
and exit-status policy are upstream. The small Host patch:

- accepts an injected argv iterator instead of `std::env::args_os()`;
- initializes the process-global logger only once;
- resets invocation-scoped error state;
- returns write failures instead of terminating the embedding process.

YourShell's adapter defines no ripgrep flags or search behavior.
