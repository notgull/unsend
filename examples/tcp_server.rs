// SPDX-License-Identifier: LGPL-3.0-or-later OR MPL-2.0
// This file is a part of `unsend`.
//
// `unsend` is free software: you can redistribute it and/or modify it under the
// terms of either:
//
// * GNU Lesser General Public License as published by the Free Software Foundation, either
//   version 3 of the License, or (at your option) any later version.
// * Mozilla Public License as published by the Mozilla Foundation, version 2.
// * The Patron License (https://github.com/notgull/unsend/blob/main/LICENSE-PATRON.md)
//   for sponsors and contributors, who can ignore the copyleft provisions of the above licenses
//   for this project.
//
// `unsend` is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
// PURPOSE. See the GNU Lesser General Public License or the Mozilla Public License for more
// details.
//
// You should have received a copy of the GNU Lesser General Public License and the Mozilla
// Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/>

//! A naive TCP server.

use async_io::Async;
use blocking::{unblock, Unblock};
use futures_lite::prelude::*;

use std::cell::Cell;
use std::fs::File;
use std::net::TcpListener;

use unsend::channel::channel;
use unsend::executor::Executor;

fn main() {
    async_io::block_on(async {
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
    });
}
