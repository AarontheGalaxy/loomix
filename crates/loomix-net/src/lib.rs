//! Network audio protocol. Populated at M11: packet parsing is attack
//! surface, so the fuzz targets from spec layer 6 land with the parser, not
//! after it.
#![forbid(unsafe_code)]
