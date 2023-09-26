// SPDX-License-Identifier: LGPL-3.0-or-later OR MPL-2.0
// This file is a part of `unsend`.
//
// `unsend` is free software: you can redistribute it and/or modify it under the
// terms of either:
//
// * GNU Lesser General Public License as published by the Free Software Foundation, either
//   version 3 of the License, or (at your option) any later version.
// * Mozilla Public License as published by the Mozilla Foundation, version 2.
//
// `unsend` is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
// PURPOSE. See the GNU Lesser General Public License or the Mozilla Public License for more
// details.
//
// You should have received a copy of the GNU Lesser General Public License and the Mozilla
// Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/>.

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    if let Err(e) = rt.block_on(main2()) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

async fn main2() -> Result<(), Box<dyn std::error::Error>> {
    let data = Arc::new(Mutex::new(0));

    // Bind to a port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000").await?;

    // Wait for incoming connections.
    println!("Listening on {:?}", listener.local_addr()?);
    loop {
        let (conn, _) = listener.accept().await?;

        // Spawn a new task.
        let data = data.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(conn, &data).await {
                eprintln!("Error: {}", e);
            }
        });
    }
}

async fn handle_connection(
    mut conn: tokio::net::TcpStream,
    data: &Mutex<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let new_index = {
        let mut data = data.lock().unwrap();
        *data += 1;
        *data
    };

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
        let response = format!("Hello, world! {}", new_index);
        conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: ")
            .await?;
        conn.write_all(response.len().to_string().as_bytes())
            .await?;
        conn.write_all(b"\r\n\r\n").await?;
        conn.write_all(response.as_bytes()).await?;
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
