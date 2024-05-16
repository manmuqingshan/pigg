# pigg - Raspberry Pi GPIO GUI

A GUI for visualization/control of GPIO on Raspberry Pis.

## Chosen Tech

* rust
* iced for GUI
* [rppal](https://github.com/golemparts/rppal). for Raspbery Pi GPIO control

## Basic / Initial Functionality

* visual representation of the GPIO connector/header with pins with numbers and names
* able to config each pin (input, output, pulled up/down, pwm etc)
* able to set status of outputs
* able to see the status of inputs
* Able to load a config from file, and save the config that is currently set in the GUI

## Next batch of functionality

* Able to provide a time-view of inputs, so like an analyzer...


## Further out ideas

* trigger a script or WebAssembly plugin on an input event (edge, level, etc)
* able to have UI on different device to where GPIO is and connect remotely
* hence able to connect the native UI to a remote device, where some "agent" is running
* have an "agent" able to run on a Pi Pico
* Have a web UI able to connect to an agent on a Pi or Pico

## Project Structure
### PIGGUI ("Piggy")
A binary that shows a GUI using Iced.
On Raspberry pi it will include GPIO 8via rppal).
On macOS and linux it will just have the UI, without GPIO.

### PIGLET ("Piglet)
A headless binary that is only built on RaspberryPi and that has no UI.

## Building and Running
### Pre-requisites
We use "cross" to cross compile for Raspberry Pi from Linux or macOS.
Install docker or podman and "cross" for cross compiling rust on your host for the Raspberry Pi.

### Building on host development machine
Run `"make"` on macos or linux (or in fact RPi also) host to build these binaries:
* `target/debug/piggui` - GUI version without GPIO, to enable UI development on a host
* `target/aarch64-unknown-linux-gnu/release/piggui` - GUI version for Pi with GPIO, can be run natively from RPi command line
* `target/aarch64-unknown-linux-gnu/release/piglet` - Headless version for Pi with GPIO, can be run natively from RPi command line

Use `"make run"` to start `piggui` on the local machine - for GUI development.

### Building for Pi from macos or linus
If you use `make` that builds for local host AND pi (using cross).

#### Helper Env vars
There are a couple of env vars that can be setup to help you interact with your pi.

You can set these up in your env so you always have them, or set them on the command line when invoking `make`

* `PI_TARGET` Which Pi to copy files to and ssh into
`PI_TARGET := pizero2w0.local`
 
* `PI_USER` The username of your user on the pi, to be able to copy files and ssh into it
`PI_USER := andrew`

#### Make targets
* Use `make` to run `clippy`, build for the Pi using `cross`, build for the local machine using `cargo` and to run tests
* Use `make pibuild` to build only for the Pi. This will build both `piggui` (with GUI and GPIO) and `piglet` binary with GPIO only
* Use `make copy` to copy the built binaries to your raspberry pi.
* Use `make ssh` to ssh into your Pi to be able to run the binaries.

### Building for Pi on a Pi!
You should be able to use `make build` or `make run` directly, and it will build `piggui` with a GUI 
### Building for Linux/macOS
Use "make build"

## Running it
### Piggui

Piggui takes an optional filename argument, to attempt to load the code from. If there is an error
loading a config, the default config will be used.