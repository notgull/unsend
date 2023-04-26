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

//! A naive TCP client to go along with tcp_server.

use async_io::Async;
use futures_lite::prelude::*;

use std::net::TcpStream;

fn main() {
    futures_lite::future::block_on(async {
        let mut client = Async::<TcpStream>::connect(([127, 0, 0, 1], 3000))
            .await
            .unwrap();
        let number = 42i32;

        client.write_all(&number.to_be_bytes()).await.unwrap();

        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();

        let response = i32::from_be_bytes(buf);
        println!("Response: {}", response);
    });
}
