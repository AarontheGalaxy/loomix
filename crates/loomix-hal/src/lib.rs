//! CoreAudio integration. Populated starting M4 (device enumeration, hog
//! mode, clock master selection, drift-corrected resampling). One of the two
//! places in the workspace allowed to use `unsafe` (spec section 4.2).
