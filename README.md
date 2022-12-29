# unsend

`unsend` is a thread-unsafe runtime for thread-unsafe people.

## Why?

Why would you want to use `unsend`? Most `async` runtimes are thread-safe, which means that they can be used from multiple threads. In some cases, this is what you want. You have all of this hardware, after all; why not use it? However, there are a handful of reasons why you might want to use a thread-unsafe runtime instead:

- You're dealing with data that is `!Send`.
- You want to avoid involving the standard library or the operating system.
- In certain cases, the overhead of thread-safe runtimes can be significant, especially in I/O-heavy systems. You may want to try a thread-unsafe runtime to see if it improves performance.

## What?

`unsend` provides the following thread-unsafe utilities:

- Various flavors of channels.
- Task abstraction.
- A single-threaded executor.

## What not?

`unsend` does not provide the following:

- Future, stream and I/O combinators. Consider using [`futures-lite`](https://crates.io/crates/futures-lite) or [`futures`](https://crates.io/crates/futures) instead.
- Asynchronous I/O reactor or timers. It is best to have a single centralized I/O reactor, so you should use [`async-io`](https://crates.io/crates/async-io) or [`tokio`](https://crates.io/crates/tokio) instead.

## License

`unsend` is dual-licensed under the MIT License and the Apache 2.0 license.
