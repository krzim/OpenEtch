# HTTPS for OpenEtch on the Raspberry Pi

OpenEtch works over plain http (`http://192.168.1.20:7878`). That is enough for the editor, framing, Start and STOP, the
full-screen toggle, Safari's **File > Add to Dock**, **Add to Home Screen** on iPad and iPhone, and Chrome/Edge's
**Create shortcut > Open as window**.

Browsers keep some features for secure origins (`https://`, or `localhost` on the same computer):

- installing OpenEtch as an app in Chrome, Edge and Android, and the title-bar layout (window-controls-overlay)
- the browser camera (Camera > This browser's camera)
- Screen Wake Lock (keeping an iPad or phone awake while you watch a burn)
- Keyboard Lock (Esc stays with the app in full screen instead of leaving it)
- service workers, and the `Sec-Fetch-Site` header the engine uses to check one-click Start

The engine has no TLS of its own (engine-native `--tls-cert/--tls-key` is planned, not built). Put a proxy in front of
it, on the Pi. Two routes follow; the first is the recommended one. In both, the engine moves to loopback and keeps
requiring its access token with `--require-token`, because the proxy reaches it over 127.0.0.1 on behalf of every
other device (on loopback the engine otherwise asks for no token).

Do **not** use a plain self-signed certificate that you click through: Chrome then refuses to install the app or run
a service worker, and iOS Home Screen apps will not load it.

## The access token first

Whatever the route, every device needs the engine's access token once. The engine keeps it in
`<config-dir>/access-token` (mode 0600) and reuses it after restarts and updates.

```sh
openetch token --config-dir /home/pi/.config/openetch
```

prints the token and a pairing link (`http://<pi>:7878/?token=…`). Open the link on a device, or paste the token (or
the link) into the app's **Connect to OpenEtch** card. `openetch token --rotate` makes a new token: restart the engine,
then pair every device again. Use the same `--config-dir` as the service.

Opening the pairing link also sets an HttpOnly login cookie, so a Safari **Add to Dock** app made afterwards opens
already connected. iOS Home Screen apps have storage of their own and ask for the token once.

## Route (a): Tailscale (recommended)

No engine code and nothing to install in any trust store: Tailscale gets a publicly trusted Let's Encrypt
certificate for the Pi's tailnet name and renews it. Every device that opens OpenEtch needs the Tailscale app running
(macOS, Windows, iPadOS, iOS and Android all have one; on iOS it uses the VPN slot). Traffic on the LAN still goes
directly between the devices over WireGuard. It also gives remote access, which is acceptable because a burn only
starts from Start in the app or the foot switch, and STOP needs no token.

1. Install Tailscale on the Pi and log in (enable MagicDNS and HTTPS certificates in the tailnet's admin console):

   ```sh
   curl -fsSL https://tailscale.com/install.sh | sh
   sudo tailscale up
   ```

2. Serve the engine over HTTPS on the tailnet:

   ```sh
   sudo tailscale serve --bg --https=443 http://127.0.0.1:7878
   ```

3. Change the engine's service to listen on loopback, require the token and accept the tailnet name
   (`<pi-name>.<tailnet>.ts.net`, as `tailscale status` shows it):

   ```ini
   [Service]
   ExecStart=/home/pi/openetch/openetch --usb --listen 127.0.0.1:7878 --require-token --allow-host <pi-name>.<tailnet>.ts.net --config-dir /home/pi/.config/openetch --shutdown-grace-secs 3600
   TimeoutStopSec=3660
   KillMode=mixed
   ```

   then `sudo systemctl daemon-reload` and restart it with `tools/pi/safe-restart.sh` (it waits for a running burn).

4. On each device, with the Tailscale app running, open `https://<pi-name>.<tailnet>.ts.net` and paste the token (or
   open `https://<pi-name>.<tailnet>.ts.net/?token=<token>`). Then use **Help > Use as an app** to install it.

To keep plain http on the LAN as well, leave `--listen 0.0.0.0:7878` instead of loopback; the token is required there
anyway, and `--require-token` is then not needed.

## Route (b): Caddy with its own certificate authority

No Tailscale, but the Caddy root certificate has to be trusted once on every device.

1. Install Caddy on the Pi (`sudo apt install caddy`) and put this in `/etc/caddy/Caddyfile`:

   ```
   openetch.local, 192.168.1.20 {
       tls internal
       reverse_proxy 127.0.0.1:7878
   }
   ```

   `sudo systemctl reload caddy`. Caddy proxies the event WebSocket and compresses on its own.

2. Run the engine on loopback with the token required and the name allowed:
   `--listen 127.0.0.1:7878 --require-token --allow-host openetch.local` (as in route (a), step 3).

3. Copy Caddy's root certificate from the Pi,
   `/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt`, and trust it on each device:
   - **macOS:** open it in Keychain Access (System keychain), then set Trust to **Always Trust**.
   - **Windows:** install it into **Trusted Root Certification Authorities** (Local Machine).
   - **iPad / iPhone:** AirDrop or mail it to the device, install the profile (Settings > General > VPN & Device
     Management), then turn it on under **Settings > General > About > Certificate Trust Settings**.
   - **Android:** Settings > Security > Encryption & credentials > Install a certificate > CA certificate.
   - **Firefox** uses the system's roots on Windows and macOS by default.

4. Open `https://openetch.local` (or `https://192.168.1.20`) and pair the device as above. `openetch.local` needs
   mDNS (avahi on the Pi, which Raspberry Pi OS runs by default).

## Stopgap for your own computer: a Chrome/Edge flag

On the owner's own PC or Mac, Chrome and Edge can treat the Pi's http address as secure, with no infrastructure:
open `chrome://flags` (or `edge://flags`), find **Insecure origins treated as secure**, enter
`http://192.168.1.20:7878`, enable it and relaunch. Install, the title-bar layout, the camera and Wake Lock then work
in that browser profile. It does nothing for other devices or browsers.

## Checking it

```sh
curl -sI https://<host>/manifest.webmanifest   # 200, content-type: application/manifest+json
curl -sI https://<host>/assets/nope.js         # 404
curl -s -o /dev/null -w '%{http_code}\n' https://<host>/api/machine   # 401 without the token
curl -s -H "Authorization: Bearer <token>" https://<host>/api/machine # the machine status
```

In the app, **Help > Use as an app** shows the steps for the browser you are in, and no longer shows the HTTPS note
once the page is secure.
