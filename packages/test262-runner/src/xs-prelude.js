// XS+SES prelude: sets up the SES lockdown shim for XS and exposes
// pass-style bytes helpers as globals for the pass-style-bytes parity tests.
// XS ships native Compartment and hardening support; the conditional
// exports in ses/lockdown-shim.js resolve to src-xs/lockdown-shim.js
// when the "xs" condition is active, adapting those to the SES API shape.
// The shared interlude that installs the bytes globals lives in
// expose-pass-style-bytes-globals.js so it stays identical to node-prelude.js.
import 'ses/lockdown-shim.js';
import './expose-pass-style-bytes-globals.js';
