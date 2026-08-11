# experiments

Throwaway probes that answered a question once. Each entry says what was asked, what was run, and
what the answer turned out to be — kept because the answer is cheap to lose and expensive to
re-derive.

## 001 — does an empty argv reach `activate`?

**Question:** `Application::run_with_args(&[])` reads as "no arguments". Is it?

**Answer: no, and the failure is silent.** argv[0] is the program name, so an empty vector is
malformed to GApplication: it returns without ever emitting `activate`. The symptom is a process
that starts, initialises GTK far enough to probe Vulkan, and exits with no window and no error.
Cost one iteration of chasing layer-shell, which was never involved. Use `run()`.

## 002 — is `GSK_RENDERER=cairo` viable on a GPU-less console?

**Question:** the target console has no render node and no GPU userspace at all — not even a
software GL driver. Does GTK4 work there?

**Answer: yes, and it is already proven in production.** GTK 4.22 still ships `GskCairoRenderer`
and has not scheduled it for removal. Two layer-shell clients already run on that exact session
under `GSK_RENDERER=cairo`. This is what ruled out every Rust-native toolkit: they need GL.
