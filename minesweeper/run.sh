#!/bin/sh

export RUSTFLAGS='-C target-feature=+crt-static'

cd minesweeper

echo "building minesweeper servers ..."
cargo build --target x86_64-unknown-linux-gnu --bin minesweeper2d

cargo build --target x86_64-unknown-linux-gnu --bin minesweeper3d

cargo build --target x86_64-unknown-linux-gnu --bin infinite_sweeper

echo "starting minesweeper in busybox"
podman run --rm -it --name minesweeper2d --pod bits-server \
	-v ./target/x86_64-unknown-linux-gnu/debug/minesweeper2d:/minesweeper2d \
	-v ../../out/worlds/christmas_tree:/world --replace \
	busybox /minesweeper2d

echo "starting ms3d in busybox"
podman run --rm -it --name minesweeper3d --pod bits-server \
	-v ./target/x86_64-unknown-linux-gnu/debug/minesweeper3d:/minesweeper3d \
	-v ../../out/worlds/christmas_tree:/world --replace \
	busybox /minesweeper3d

echo "starting infinite sweeper in busybox"
podman run --rm -it --name infinite_sweeper --pod bits-server \
	-v ./target/x86_64-unknown-linux-gnu/debug/infinite_sweeper:/infinite_sweeper --replace \
	busybox /infinite_sweeper

cd ..
