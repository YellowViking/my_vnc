## Overview

Why another VNC client?
- There's no open source rust VNC server implementation
- Windows based VNC servers such as TightVNC and UltraVNC don't support D3D based screen capture
- I need a VNC server that allows tunneling over websockets for restricted environments (Secure Workstations, etc)
- I also need a win32 binary that can be run as dll using `rundll32.exe` to bypass application whitelisting

## Features
- [x] D3D based screen capture (or GDI based screen capture if D3D is not available)
- [x] Websocket tunneling
- [x] Win32 binary that can be run as dll using `rundll32.exe`
- [x] Support for multiple clients
- [x] Support for multiple encodings (Raw, ZRLE)

## Compoments
- [x] winvnc-tunnel: regular VNC server
- [x] my_vnc.dll: VNC server that can be run as dll using `rundll32.exe`
- [x] winvnc-tunnel: VNC proxy server that tunnels VNC over websockets (listening on vnc port and websocket port)

## Usage
### winvnc-tunnel
```
./winvnc-tunnel.exe 

it listens on port 80 for websocket connections and port 5900 for VNC connections
```

### my_vnc.dll
```
rundll32.exe my_vnc.dll,PrintUIEntry,tunnelhost

with the default settings
Args {
    use_tunnelling: true,
    host: host.to_string(),
    port,
    display: 0,
    use_gdi: true,
    enable_profiling: true,
},
```

### winvnc-server
```
Usage: winvnc-server.exe [OPTIONS]

Options:
      --host <HOST>        [default: localhost]
  -p, --port <PORT>        [env: PORT=] [default: 5900]
  -d, --display <DISPLAY>  [env: DISPLAY=] [default: 0]
  -t, --use-tunnelling     [env: USE_TUNNELLING=]
  -g, --use-gdi            [env: USE_GDI=]
  -e, --enable-profiling   [env: ENABLE_PROFILING=]
  -h, --help               Print help (see more with '--help')
  -V, --version            Print version

```