# FEL reference image provenance

- `chip-fel-jumper.png`: supplied by the project owner in this task on 2026-09-12,
  copied without editing. Depicts bare CHIP with a FEL-to-GND header jumper.
  The same setup appears in the [manufacturer's CHIP documentation](https://docs.getchip.cc/chip#fel-mode).
  No new ownership or general reuse license is asserted for the supplied photograph.
- `pocketchip-top-fel.svg`: original project schematic, not a photograph. Depicts
  only the last four labeled top pads (UART RX, UART TX, GROUND, FEL); spacing,
  enclosure and keyboard are simplified, and other pads are omitted. The illustrated
  jumper connects GROUND and FEL. Labels/orientation were checked on 2026-09-12 against
  the [manufacturer's GPIO image](https://docs.getchip.cc/images/gpio.jpg) and
  [askaye's Pocket C.H.I.P. teardown photo on iFixit](https://guide-images.cdn.ifixit.com/igi/SchYaRQPxFFcdYBS.huge)
  ([guide](https://www.ifixit.com/Teardown/Pocket+C.H.I.P.+Teardown/167908)).
  Those reference photos are linked, not bundled or modified.

The schematic is documentation of the pad connection, not evidence of a working
flashing session. Physical testing is still required.
