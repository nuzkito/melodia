# Troubleshooting

This file lists common problems and their solutions. Back to [README.md](../README.md).

| Problem | Solution |
| --- | --- |
| `INVALID_CLIENT: Invalid redirect URI` in the browser | Register exactly `http://127.0.0.1:8898/callback` in your Spotify app. |
| “Your account isn't authorized in the Spotify app” | Add your email in *User Management* in the Dashboard. |
| “No Spotify device is active” | Select “Play in this app” in the device selector. Or open Spotify on a different device. |
| “Could not open port 8898” | Close the other copy of the app. Or use a different port with `MELODIA_PORT`, and register that port in Spotify. |
| “Spotify can't play in this WebView (DRM)…” | Spotify does not accept the DRM of WebView2 (Widevine). Use a different device. |
| “The web player never connected to Spotify…” | The SDK does not respond. Some reports exist for apps in development mode. |
| “Spotify doesn't allow playback with this account…” | The account is not Premium. |
| “Could not start WebView2…” | Install the WebView2 runtime. Microsoft provides it for free on its website. |
| The window is blank or does not start | The app tries OpenGL again. To use OpenGL at once, set `MELODIA_RENDERER=glow`. |
| The window stops updating during a resize | Set `MELODIA_RENDERER=glow`. |
| Any other problem | Read the log file (see below). |

The log file is `%APPDATA%\Melodia\config\melodia.log`.
