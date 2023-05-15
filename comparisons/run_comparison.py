# SPDX-License-Identifier: LGPL-3.0-or-later OR MPL-2.0
# This file is a part of `unsend`.
#
# `unsend` is free software: you can redistribute it and/or modify it under the
# terms of either:
#
# * GNU Lesser General Public License as published by the Free Software Foundation, either
#   version 3 of the License, or (at your option) any later version.
# * Mozilla Public License as published by the Mozilla Foundation, version 2.
# * The Patron License (https://github.com/notgull/unsend/blob/main/LICENSE-PATRON.md)
#   for sponsors and contributors, who can ignore the copyleft provisions of the above licenses
#   for this project.
#
# `unsend` is distributed in the hope that it will be useful, but WITHOUT ANY
# WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
# PURPOSE. See the GNU Lesser General Public License or the Mozilla Public License for more
# details.
#
# You should have received a copy of the GNU Lesser General Public License and the Mozilla
# Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/>.

# Run comparisons between different async runtimes running a basic HTTP server.

import subprocess as sp
import os
import matplotlib.pyplot as plt

def processWrkOutput(output):
    """Processes the output of the `wrk` command"""

    # Split output by newlines.
    lines = output.split("\n")

    # Get the line with "Req/Sec" in it
    reqSecLine = [line for line in lines if "Req/Sec" in line][0]
    print(reqSecLine)

    # Split by whitespace and filter out empty strings.
    reqSecLine = [item for item in reqSecLine.split(" ") if item != ""]

    # Get the average number of requests per second
    reqSec = reqSecLine[1]

    # If it ends with "k", multiply by 1000.
    if reqSec.endswith("k"):
        reqSec = float(reqSec[:-1]) * 1000
    else:
        reqSec = float(reqSec)

    # Return the average number of requests per second.
    return reqSec

def runWrkForPackage(package):
    """Runs the `wrk` command for the given package"""

    print(f"Running wrk for {package}...")

    # Run the "cargo build --release" command for the package.
    sp.run(["cargo", "build", "--release", "-p", package])

    # In the background, use cargo run to run the package.
    process = sp.Popen(["cargo", "run", "--release", "-p", package], stdout=sp.PIPE, stderr=sp.PIPE)

    # Run the wrk command.
    wrk = sp.run(["wrk", "-t12", "-c400", "-d2s", "http://localhost:8000/index.html"], stdout=sp.PIPE, stderr=sp.PIPE)

    # Kill the process.
    process.kill()

    # Return the output.
    output = wrk.stdout.decode("utf-8")
    return processWrkOutput(output)

def runCollectionAndPlot(packages, outPath, title):
    """Runs the list of packages and plots them"""

    # Run the wrk command for each package.
    results = [runWrkForPackage(package) for package in packages]

    # Plot the results.
    plt.bar(packages, results)
    plt.ylabel("Requests per second")
    plt.title(title)
    plt.savefig(outPath)

    # Clear the plot.
    plt.clf()

HELLO = ["unsend-hello", "tokio-hello", "tokio-local-hello", "smol-hello", "smol-local-hello"]
LOCK = ["unsend-lock", "tokio-lock", "tokio-local-lock", "smol-lock", "smol-local-lock"]

def main():
    """Main function"""

    # Create the output directory.
    os.makedirs("out", exist_ok=True)

    # Run the hello world example.
    runCollectionAndPlot(HELLO, "out/hello.png", "Hello world HTTP server")
    runCollectionAndPlot(LOCK, "out/lock.png", "Locking HTTP server")

if __name__ == "__main__":
    main()

