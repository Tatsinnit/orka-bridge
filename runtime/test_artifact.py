import importlib.util
import pathlib
import struct
import unittest

spec=importlib.util.spec_from_file_location("artifact",pathlib.Path(__file__).with_name("artifact.py"))
api=importlib.util.module_from_spec(spec);spec.loader.exec_module(api)

class Artifact(unittest.TestCase):
 def header(self,machine):
  b=bytearray(64);b[:6]=b"\x7fELF\x02\x01";struct.pack_into("<H",b,18,machine);return bytes(b)
 def test_both_static_architectures(self):
  for target,m in api.TARGETS.items():self.assertEqual(m,api.inspect(self.header(m),"no dynamic dependencies",target))
 def test_interpreter_needed_wrong_arch_and_not_elf(self):
  for h,report in [(self.header(62),"INTERP"),(self.header(62),"(NEEDED)"),(self.header(183),""),(b"not-elf","")]:
   with self.assertRaises(ValueError):api.inspect(h,report,"x86_64-unknown-linux-musl")
if __name__=="__main__":unittest.main()
