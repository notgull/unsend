# unsend

A thread unsafe runtime for thread unsafe people.

Most contemporary `async` runtimes are thread safe, as they are meant to be used in networking applications where multithreading is all but necessary. This kind of hardware parallelism improves the performance of parallel programs. However, you may want to avoid this kind of synchronization instead. Reasons for this include:

- You are dealing with data that is `!Send` and therefore cannot be shared between threads.
- You want to avoid including the standard library or the operating system.
- You are running on embedded hardware that does not support multithreading.
- You want to avoid the overhead of synchronization for programs that aren't as parallel. For instance, if your process relies on heavily mutating shared data structures, synchronization may cause more harm than good.

Applications like NodeJS and Redis take advantage of thread unsafety in this way. In these cases, it may be worth it to use `unsend` and avoid the issues with synchronization.

`unsend` provides the following utilities:

- Equivalents to synchronization primitives, like `Mutex` and `RwLock`, but not `Sync` and with no synchronization code.
- A multi-producer multi-consumer unbounded channel.
- A single-threaded executor.

## The Caveats

Most types in this crate, such as the synchronization primitives and channel, involve no synchronization primitives whatsoever. There are no atomics, mutexes or anything that is multiprocessing-aware.

However, with executors, this becomes significantly more complicated. `Waker` needs to be `Send + Sync`, meaning that the internal scheduling function has to be thread safe. By default, the executor uses a thread-aware atomic channel to store tasks. However, if the `std` feature is enabled, the `Waker` can detect whether it was woken up from the same thread that it was created in. If this is the case, the executor will use a thread-unsafe channel instead.

## Philosophy

- Use lightweight tasks instead of heavy-weight threads.
- Share data through thread-unsafe channels instead of thread-safe ones.
- Simplicity over complexity.

## Features

All features are enabled by default.

- `alloc` enables usage of the global allocator in the `alloc` crate, enabling the usage of channels.
- `executor` brings in more dependencies and enables the `Executor` type. Requires `alloc`.
- `std` enables use of he standard library, enabling certain optimizations.

## Out of Scope

Unlike other `async` runtimes, `unsend` deliberately avoids providing certain popular features. Some of these features include:

- Future and stream combinators. The [`futures-lite`] and [`futures`] crates provide many more than we could ever provide, and most of them work with thread-unsafe types by default.
- Thread pooling. As we avoid synchronization in most cases, thread pooling goes against the philosophy of this crate. The [`blocking`] crate provides a good `async`-aware thread pool.
- An I/O reactor. [`async-io`] provides a decent, minimal reactor that works nearly everywhere. The [`tokio`] crate provides a reactor as well. However, in the future it may be a good idea to provide this, as the [`async-io`] reactor involves a substantial amount of synchronization. Please open a PR if you would like to see this feature.

[`futures-lite`]: https://crates.io/crates/futures-lite
[`futures`]: https://crates.io/crates/futures
[`blocking`]: https://crates.io/crates/blocking
[`async-io`]: https://crates.io/crates/async-io
[`tokio`]: https://crates.io/crates/tokio

## Minimum Supported Rust Version

The Minimum Supported Rust Version (MSRV) of this crate is **1.48**. As a **tentative** policy, the MSRV will not advance past the [current Rust version provided by Debian Stable](https://packages.debian.org/stable/rust/rustc). At the time of writing, this version of Rust is *1.48*. However, the MSRV may be advanced further in the event of a major ecosystem shift or a security vulnerability.

## Examples

A basic TCP server using `unsend`, `blocking` and `async-io`.

```rust
use async_io::Async;
use blocking::{unblock, Unblock};
use futures_lite::prelude::*;

use std::cell::Cell;
use std::fs::File;
use std::net::TcpListener;

use unsend::channel::channel;
use unsend::executor::Executor;

let (tx, rx) = channel();

// A shared value that will be mutated by the tasks.
let shared = Cell::new(1);

// Spawn a task that will read from the channel and write to a log file.
let executor = Executor::new();
executor
    .spawn(async move {
        let file = unblock(|| File::create("log.txt")).await.unwrap();
        let mut file = Unblock::new(file);

        while let Ok(msg) = rx.recv().await {
            let message = format!("Sent out: {}", msg);
            file.write_all(message.as_bytes()).await.unwrap();
        }
    })
    .detach();

executor
    .run(async {
        loop {
            // Listen for incoming connections.
            let listener = Async::<TcpListener>::bind(([0, 0, 0, 0], 3000)).unwrap();

            // Accept a new connection.
            let (mut stream, _) = listener.accept().await.unwrap();

            // Spawn a task that will operate on the stream.
            let tx = tx.clone();
            let shared = &shared;
            executor
                .spawn(async move {
                    // Read a 4-byte big-endian integer from the stream.
                    let mut buf = [0; 4];
                    stream.read_exact(&mut buf).await.unwrap();
                    let value = u32::from_be_bytes(buf);

                    // Multiply it by the shared value.
                    let value = value * shared.get();

                    // Increment the shared value.
                    shared.set(shared.get() + 1);

                    // Write the value to the stream.
                    stream.write_all(&value.to_be_bytes()).await.unwrap();

                    // Send the value to be logged.
                    tx.send(value).unwrap();
                })
                .detach();
        }
    })
    .await;
```

## Credits

Parts of this crate are based on [`smol`] by Stjepan Glavina.

[`smol`]: https://crates.io/crates/smol

## License

`unsend` is free software: you can redistribute it and/or modify it under the terms of
either:

* GNU Lesser General Public License as published by the Free Software Foundation, either
version 3 of the License, or (at your option) any later version.
* Mozilla Public License as published by the Mozilla Foundation, version 2.
* The [Patron License](https://github.com/notgull/unsend/blob/main/LICENSE-PATRON.md) for [sponsors](https://github.com/sponsors/notgull) and [contributors](https://github.com/notgull/async-winit/graphs/contributors), who can ignore the copyleft provisions of the GNU AGPL for this project.

`unsend` is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
See the GNU Lesser General Public License or the Mozilla Public License for more details.

You should have received a copy of the GNU Lesser General Public License and the Mozilla
Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/> or
<https://www.mozilla.org/en-US/MPL/2.0/>.