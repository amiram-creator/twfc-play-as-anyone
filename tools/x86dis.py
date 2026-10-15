"""Disassemble TWFC.exe at virtual addresses (ImageBase 0x10900000).

usage: python x86dis.py <TWFC.exe> <hexVA:hexLen> [...]   (needs: pip install pefile capstone)
"""
import sys, pefile, capstone
pe = pefile.PE(sys.argv[1], fast_load=True)
md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
for spec in sys.argv[2:]:
    a, n = spec.split(":"); a = int(a, 16); n = int(n, 16)
    code = pe.get_data(a - 0x10900000, n)
    print(f"--- {a:08X}")
    for i in md.disasm(code, a):
        print(f"{i.address:08X}  {i.mnemonic} {i.op_str}")
