/* MXChip AZ3166 - STM32F412RG (1 MB flash, 256 KB RAM)
 *
 * We link at the flash base and boot directly, with no bootloader in the way.
 *
 * The stock MXChip bootloader normally sits here and chain-loads an app at
 * 0x0800C000, but it refuses to hand off to a non-MXChip image - the board just
 * sits in the bootloader forever. Owning flash from 0x08000000 avoids it.
 * ./restore-factory.sh puts MXChip's bootloader and app back whenever you want.
 */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 1024K
  RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
