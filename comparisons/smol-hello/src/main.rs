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
// Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/>.

use smol::io::BufReader;
use smol::prelude::*;
use smol::Executor;

use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

fn main() {
    if let Err(e) = smol::block_on(main2()) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

async fn main2() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new executor.
    let executor = Arc::new(Executor::new());

    // Bind to a port.
    let listener = smol::Async::<TcpListener>::bind(([127, 0, 0, 1], 8000))?;

    // Wait for incoming connections.
    println!("Listening on {:?}", listener.get_ref().local_addr()?);

    // Spawn a bunch of threads to run on.
    let (_signal, shutdown) = smol::channel::bounded::<()>(1);
    for _ in 0..thread::available_parallelism()?.get() {
        let shutdown = shutdown.clone();
        let executor = executor.clone();

        thread::spawn(move || {
            let _ = smol::block_on({
                // Run the executor.
                executor.run(shutdown.recv())
            });
        });
    }

    let incoming = listener.incoming();
    smol::pin!(incoming);
    executor
        .run(incoming.try_for_each(|conn| {
            conn.map(|conn| {
                // Spawn a new task.
                let task = executor.spawn(async move {
                    if let Err(e) = handle_connection(conn).await {
                        eprintln!("Error: {}", e);
                    }
                });

                // Detach the task.
                task.detach();
            })
        }))
        .await?;

    Ok(())
}

async fn handle_connection(
    mut conn: impl AsyncRead + AsyncWrite + Unpin,
) -> Result<(), Box<dyn std::error::Error>> {
    // Read HTTP headers until we get to "\r\n\r\n"
    let mut data = Vec::new();
    let mut buf_reader = BufReader::new(&mut conn);
    loop {
        let len = buf_reader.read_until(b'\n', &mut data).await?;
        if len == 0 {
            // EOF
            break;
        }

        // If we got the end of the headers, stop.
        if data.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    // Parse the result to get the body using httparse.
    let mut headers = [httparse::EMPTY_HEADER; 16];
    let mut req = httparse::Request::new(&mut headers);

    // Find the Content-Length header.
    req.parse(&data)?;
    let body_length = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("Content-Length"))
        .map(|h| std::str::from_utf8(h.value).unwrap().parse::<usize>())
        .transpose()?
        .unwrap_or(0);

    if body_length == 0 {
        // Just send a hello world.
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\nHello world!")
            .await?;
        return Ok(());
    }

    // Read the body.
    let mut body = Vec::new();
    buf_reader
        .take(body_length as u64)
        .read_to_end(&mut body)
        .await?;

    // Write an HTTP response containing the body.
    conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: ")
        .await?;
    conn.write_all(body.len().to_string().as_bytes()).await?;
    conn.write_all(b"\r\n\r\n").await?;
    conn.write_all(&body).await?;

    Ok(())
}
