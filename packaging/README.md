# ApexEdge Packaging

This directory is the release packaging scaffold for v1.0 GA.

- Windows: build an MSI with WiX around the `apex-edge.exe` release binary.
- Linux: build `.deb` and `.rpm` packages around the `apex-edge` release binary.
- macOS: build a `.pkg` around the signed `apex-edge` release binary.
- First run: execute `apex-edge init` to create the database, apply migrations, load or generate the audit key, and print setup details.

The packaging jobs should call the same local quality gates as CI before producing artifacts.
