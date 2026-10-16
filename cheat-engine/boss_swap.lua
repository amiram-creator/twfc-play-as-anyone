-- TWFC runtime boss swap (Cheat Engine Lua): the research prototype the standalone mod (omega_mod) was ported from.
-- See docs/TECHNICAL.md. Load it in Cheat Engine's Lua console while attached to TWFC.exe:
--   dofile([[<repo>\cheat-engine\boss_swap.lua]])
--   print(omega_before_respawn())   -- then die and restart from checkpoint, then: print(omega_after_respawn())
-- Only memory is changed; restarting the game undoes everything.
-- Addresses: exe MD5 69acde90af160650ee5f60fff2f57d50, ImageBase 0x10900000 (no ASLR).

local GNAMES, GOBJECTS = 0x12B2F0F4, 0x12B2F0A4
local CID_CHASSIS, CID_ROBODEF = 1743, 1879     -- TnDataProvider_Chassis, TnTransformer
local CHASSIS_OBJECTPATH = 0x78                 -- FString in the chassis provider

-- Player-side ROBODEF fields that boss ROBODEFs leave empty (offset -> what Megatron has there).
-- Boss mesh/physics/animtree (+0x64/+0x68/+0x6C), size (+0x7C/+0x80) and effects stay the boss's own.
local GRAFT = {
  0xA4,  -- VehicleForm (VEHDEF)
  0xA8,  -- camera set (CAMSET)
  0xB0,  -- targeting
  0xD4,  -- acrobatics
  0xE8,  -- transform momentum
  0xF0, 0xF4, -- floats next to health (20.0, 2.0 on Megatron)
  0xF8,  -- health
  0x104, -- melee set
  0x11C, -- hit reactions
  0x138, -- interact skeleton class
  0x140, -- wounded
  0x148, 0x14C, 0x150, -- defrag material, arm part mesh/anim
  0x160, -- overshield material
  -- NOT 0x164..0x16C (BuffMaterials, a TArray header): copying it makes two ROBODEFs share one heap buffer = double free when
  -- the level unloads, crash on the next chapter load (docs/TECHNICAL.md)
}

local names = {}
local function fname(fn)
  local i, n = fn >> 14, fn & 0x3FFF
  local s = names[i]
  if not s then
    local e = readInteger(readInteger(GNAMES) + i * 4)
    s = e and readString(e + 8, 128, true) or "?"
    names[i] = s
  end
  return n > 0 and (s .. "_" .. (n - 1)) or s
end

local function cid(o) return (readInteger(o + 0x14) & 0xFFF) - 1 end

function twfc_find(classid, name)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and cid(o) == classid and fname(readInteger(o + 4)) == name then return o, i end
  end
end

local function set_fstring(addr, s)
  local mem = allocateMemory(#s * 2 + 16)
  writeString(mem, s, true); writeBytes(mem + #s * 2, 0, 0)
  writeInteger(addr, mem); writeInteger(addr + 4, #s + 1); writeInteger(addr + 8, #s + 1)
end

local function graft(b, d)
  local log = {}
  for _, off in ipairs(GRAFT) do
    if readInteger(b + off) == 0 then
      writeInteger(b + off, readInteger(d + off))
      log[#log + 1] = string.format("+%X", off)
    end
  end
  return log
end

-- chassis: "Tank" (Megatron), "Truck" (Optimus Prime), ... ; boss/donor: ROBODEF object names.
function boss_swap(chassis, boss, donor)
  local c = twfc_find(CID_CHASSIS, chassis)
  local b = twfc_find(CID_ROBODEF, boss)
  local d = twfc_find(CID_ROBODEF, donor)
  if not (c and b and d) then
    return string.format("missing: chassis=%s boss=%s donor=%s (is the boss level loaded?)", tostring(c), tostring(b), tostring(d))
  end
  local log = graft(b, d)
  -- the ROBODEF's full path: <package>.<name>, package = its outer
  local pkg = fname(readInteger(readInteger(readInteger(GOBJECTS) + ((readInteger(b + 0x24) >> 4) - 1) * 4) + 4))
  local path = pkg .. "." .. boss
  set_fstring(c + CHASSIS_OBJECTPATH, path)
  return string.format("chassis %s -> %s; grafted from %s: %s", chassis, path, donor, table.concat(log, " "))
end

-- A checkpoint restart reloads the boss level's streamed DSN package, which creates a fresh boss ROBODEF
-- with null player fields again (test 7). boss_swap_arm() re-applies the graft at the moment the spawn reads
-- the chassis ObjectPath, via a CE hardware read breakpoint on the FString data pointer (CE's debugger must be
-- free: don't run the game under x32dbg at the same time). boss_swap_log holds one line per hit.
-- Every callback runs under pcall and always returns 1: an uncaught Lua error leaves the game stopped in the
-- breakpoint (test 9, second hit on the path read).
boss_swap_log = {}
local function logf(fmt, ...)
  boss_swap_log[#boss_swap_log + 1] = os.date("%H:%M:%S ") .. string.format(fmt, ...)
end

local function safe(name, fn)
  return function()
    local ok, err = pcall(fn)
    if not ok then logf("%s callback error: %s", name, tostring(err)) end
    return 1 -- continue the game
  end
end

local function set_bp(old, addr, size, trigger, name, fn)
  if old then debug_removeBreakpoint(old) end
  debug_setBreakpoint(addr, size, trigger, bpmDebugRegister, safe(name, fn))
  return addr
end

-- Logs access violations (the game's own crash handler swallows them, and no dump is written while CE's
-- debugger is attached): a breakpoint on the 32-bit ntdll!KiUserExceptionDispatcher, where
-- [esp] = EXCEPTION_RECORD*, [esp+4] = CONTEXT* (x86 CONTEXT: Edi 9C, Esi A0, Ebx A4, Edx A8, Ecx AC, Eax B0,
-- Ebp B4, Eip B8, Esp C4). Also lists return addresses into TWFC.exe found on the faulting stack.
local function log_exception()
  local rec, ctx = readInteger(ESP), readInteger(ESP + 4)
  local code = readInteger(rec)
  if code ~= 0xC0000005 then return end
  local r = function(o) return readInteger(ctx + o) or 0 end
  logf("AV at %08X (RVA %X) info %X/%08X | EAX=%08X EBX=%08X ECX=%08X EDX=%08X ESI=%08X EDI=%08X EBP=%08X ESP=%08X",
    readInteger(rec + 0xC), (readInteger(rec + 0xC) or 0) - 0x10900000, readInteger(rec + 0x14) or 0,
    readInteger(rec + 0x18) or 0, r(0xB0), r(0xA4), r(0xAC), r(0xA8), r(0xA0), r(0x9C), r(0xB4), r(0xC4))
  local esp, rets = r(0xC4), {}
  for off = 0, 0x1000, 4 do
    local v = readInteger(esp + off)
    if v and v > 0x10901000 and v < 0x12000000 then rets[#rets + 1] = string.format("+%X:%X", off, v - 0x10900000) end
    if #rets >= 40 then break end
  end
  logf("  stack return-address candidates (offset:RVA): %s", table.concat(rets, " "))
end

function boss_swap_arm(chassis, boss, donor)
  local msg = boss_swap(chassis, boss, donor)
  local c = twfc_find(CID_CHASSIS, chassis)
  if not c then return msg end
  local cached, idx = twfc_find(CID_ROBODEF, boss)
  local d = twfc_find(CID_ROBODEF, donor) -- the donor lives in the persistent map, so it stays put

  boss_swap_bp = set_bp(boss_swap_bp, c + CHASSIS_OBJECTPATH, 4, bptAccess, "path", function()
    -- still the same live object? (a reloaded package registers a new one)
    if not (cached and readInteger(readInteger(GOBJECTS) + idx * 4) == cached) then
      cached, idx = twfc_find(CID_ROBODEF, boss)
    end
    local log = (cached and d) and graft(cached, d) or {"boss or donor not found"}
    logf("path read, boss %s: %s", cached and string.format("%08X", cached) or "nil",
      #log > 0 and table.concat(log, " ") or "already grafted")
  end)

  -- Second safety net at the point that crashed in tests 6b/7/8: the player spawn reads the camera set from
  -- [pawn+0x568]+0xA8 (pawn+0x568 = the form definition in use) at 0x10BA0A74 (RVA 0x2A0A74), ECX = that definition.
  -- If it is a ROBODEF with null player fields, graft it right there and log which object it was.
  boss_swap_cam_bp = set_bp(boss_swap_cam_bp, 0x10BA0A74, 1, bptExecute, "camera", function()
    local o = ECX
    if o == 0 then return logf("camera read: null form definition") end
    local log = (cid(o) == CID_ROBODEF and d) and graft(o, d) or {}
    logf("camera read: %08X cid=%d %s +A8=%08X; grafted: %s", o, cid(o), fname(readInteger(o + 4)),
      readInteger(o + 0xA8) or 0, #log > 0 and table.concat(log, " ") or "nothing")
  end)

  local disp = getAddressSafe("ntdll.KiUserExceptionDispatcher")
  if disp and disp < 0x100000000 then
    boss_swap_exc_bp = set_bp(boss_swap_exc_bp, disp, 1, bptExecute, "exception", log_exception)
  end
  return msg .. string.format("\nbreakpoints: path %08X, camera %08X, exceptions %s", boss_swap_bp, boss_swap_cam_bp,
    disp and string.format("%08X", disp) or "not found")
end

-- Debugger-free version (test 11): CE's debug-register breakpoints dropped callbacks (tests 9/10), and with CE's debugger
-- attached no crash dump can be taken. boss_swap_hook() injects code at the chassis ObjectPath read (0x10A801F8,
-- RVA 0x1801F8: `mov eax,[esi+68]` / `jmp 10A80202`, 5 bytes). The reverse chassis lookup there runs during the spawn,
-- after the checkpoint restart has reloaded the boss package (test 9). When ESI is our chassis (+0x10), the cave walks
-- GObjects for the boss ROBODEF (same FName + class), and if its +0xA4 is still null, copies the donor's GRAFT fields.
-- [bsw_count] counts grafts. boss_swap_unhook() restores the original bytes.
function boss_swap_hook(chassis, boss, donor)
  local msg = boss_swap(chassis, boss, donor)
  local c = twfc_find(CID_CHASSIS, chassis)
  local b = twfc_find(CID_ROBODEF, boss)
  local d = twfc_find(CID_ROBODEF, donor)
  if not (c and b and d) then return msg end
  local lines = {}
  for i, off in ipairs(GRAFT) do
    lines[#lines + 1] = string.format(
      "  cmp dword ptr [edi+%X],0\n  jne bsw_k%d\n  mov eax,[esi+%X]\n  mov [edi+%X],eax\nbsw_k%d:", off, i, off, off, i)
  end
  local script = string.format([[
alloc(bsw_cave,2048)
alloc(bsw_count,4)
registersymbol(bsw_cave)
registersymbol(bsw_count)
label(bsw_scan)
label(bsw_next)
label(bsw_done)
label(bsw_orig)
bsw_cave:
  cmp esi,%X
  jne bsw_orig
  pushad
  mov ebx,[%X]
  mov ecx,[%X]
  xor edx,edx
bsw_scan:
  cmp edx,ecx
  jae bsw_done
  mov edi,[ebx+edx*4]
  test edi,edi
  je bsw_next
  cmp dword ptr [edi+4],%X
  jne bsw_next
  mov eax,[edi+14]
  and eax,FFF
  cmp eax,%X
  jne bsw_next
  cmp dword ptr [edi+A4],0
  jne bsw_next
  mov esi,%X
%s
  inc dword ptr [bsw_count]
bsw_next:
  inc edx
  jmp bsw_scan
bsw_done:
  popad
bsw_orig:
  mov eax,[esi+68]
  jmp 10A80202
10A801F8:
  jmp bsw_cave
]], c + 0x10, GOBJECTS, GOBJECTS + 4, readInteger(b + 4), CID_ROBODEF + 1, d, table.concat(lines, "\n"))
  local ok, err = autoAssemble(script)
  if not ok then return msg .. "\nhook FAILED: " .. tostring(err) end
  return msg .. string.format("\nhook installed at 10A801F8 (cave %08X), boss FName %08X, donor %08X",
    getAddress("bsw_cave"), readInteger(b + 4), d)
end

function boss_swap_unhook()
  writeBytes(0x10A801F8, 0x8B, 0x46, 0x68, 0xEB, 0x05)
  return "original bytes restored at 10A801F8"
end

-- Camera for giants. The robot camera strategy (template CAM_Strategies_p.OverTheShoulder_STRATEGY, live copies under
-- the controller's TnCameraStrategyManager) positions the camera with TnScreenSpaceOffsetByPitchCameraBehavior.
-- Property offsets (from the property objects' packed offset field): +0x64 OffsetCurvesByPCS (TArray: data, num),
-- +0x84 CurrentOffsetCurveIndex. Each OffsetCurvesByPCS entry is 64 bytes: TArray of PCS values (12 bytes), then
-- three offset vectors (low/mid/high pitch, 9 floats), pitch min/max, smoothing. Offsets apply when the strategy
-- activates, so transform (vehicle and back) after calling this. HmOffsetAnchorPointRelativeToActorCameraBehavior
-- +0x5C is its Offset vector (Z at +0x64, raised the camera in test 12).
-- boss_camera_scale(f) multiplies all of them by f (call again with 1/f to undo); returns a summary.
local CAM_CURVES, CAM_ANCHOR = "TnScreenSpaceOffsetByPitchCameraBehavior", "HmOffsetAnchorPointRelativeToActorCameraBehavior"
function boss_camera_scale(f, fy, fz)
  -- one factor for all axes, or per axis (offsets are camera space: X = forward/back, Y = side, Z = up)
  local fa = {f, fy or f, fz or f}
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local function base(n) return (n:gsub("_%d+$", "")) end
  local curves, anchors = 0, 0
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 then
      local ok, n = pcall(function() return base(fname(readInteger(o + 4))) end)
      if ok and (n == CAM_CURVES or n == CAM_ANCHOR) then
        local oi = (readInteger(o + 0x24) >> 4) - 1
        local p = oi >= 0 and readInteger(go + oi * 4) or 0
        local pn = p ~= 0 and base(fname(readInteger(p + 4))) or ""
        -- the robot over-the-shoulder template and every live strategy copy (HmCameraStrategy_N)
        if pn == "OverTheShoulder_STRATEGY" or pn == "HmCameraStrategy" then
          if n == CAM_CURVES then
            local data, num = readInteger(o + 0x64), readInteger(o + 0x68)
            if data and data ~= 0 and num and num > 0 and num < 32 then
              for e = 0, num - 1 do
                for k = 0, 8 do local a = data + e * 64 + 12 + k * 4; writeFloat(a, readFloat(a) * fa[k % 3 + 1]) end
              end
              curves = curves + num
            end
          else
            for k = 0, 2 do local a = o + 0x5C + k * 4; writeFloat(a, readFloat(a) * fa[k + 1]) end
            anchors = anchors + 1
          end
        end
      end
    end
  end
  return string.format("scaled %d camera curves and %d anchor offsets by %g/%g/%g; transform to apply", curves, anchors, fa[1], fa[2], fa[3])
end

-- The actual camera distance (test 13): TnLocationOffsetCameraBehavior. Property offsets: +0x2C HorizontalOffset,
-- +0x30 VerticalOffset, +0x34 DefaultOrbitDistance, +0x38 OrbitDistancesByPCS (TArray of 16-byte entries:
-- PCS TArray + distance float at +0xC), +0x58 CurrentOrbitDistance, +0x5C CurrentOrbitDistanceIndex.
-- The robot over-the-shoulder camera uses DefaultOrbitDistance = 800. boss_camera_orbit(d) sets Default and
-- Current to d for the robot strategy template and its live copies (identified by Default == 800 or a previous
-- value set here). Takes effect immediately for Current; transform to re-activate if needed.
-- Safety (crash after boss_camera_giant on 2026-10-14, 21 matches instead of 9): only objects of the exact class
-- (class id taken from the OverTheShoulder template's behaviour), whose outer is a camera strategy (template or
-- live HmCameraStrategy copy), and whose current Default is the robot value (800) or the value set by the last call.
local CID_LOC_OFFSET
local function loc_offset_behaviours(match)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local found = {}
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 then
      local ok, nm = pcall(function() return fname(readInteger(o + 4)) end)
      if ok and nm:find("^TnLocationOffsetCameraBehavior_") then
        local oi = (readInteger(o + 0x24) >> 4) - 1
        local p = oi >= 0 and readInteger(go + oi * 4) or 0
        local pn = p ~= 0 and fname(readInteger(p + 4)) or ""
        if pn == "OverTheShoulder_STRATEGY" then CID_LOC_OFFSET = cid(o) end
        if pn:find("^HmCameraStrategy_") or pn == "OverTheShoulder_STRATEGY" then found[#found + 1] = o end
      end
    end
  end
  local out = {}
  for _, o in ipairs(found) do
    if cid(o) == CID_LOC_OFFSET and match(readFloat(o + 0x34)) then out[#out + 1] = o end
  end
  return out
end

boss_orbit_last = 800
function boss_camera_orbit(d)
  local last = boss_orbit_last
  local list = loc_offset_behaviours(function(def) return def == 800 or def == last end)
  for _, o in ipairs(list) do
    if readFloat(o + 0x58) == readFloat(o + 0x34) then writeFloat(o + 0x58, d) end -- only a Current that follows Default
    writeFloat(o + 0x34, d)
  end
  boss_orbit_last = d
  return string.format("orbit distance %g set on %d camera behaviours (class id %s)", d, #list, tostring(CID_LOC_OFFSET))
end

-- One call for the giant camera (test 13, made safe in session 4): orbit distance 6000, camera 2500 to the side and
-- 1500 up. Writes only the OverTheShoulder template and the live robot cameras in use (Current orbit == 800), not every
-- strategy copy (each respawn adds more). Call after the respawn and one transform cycle, then transform once more.
function boss_camera_giant(dist, h, v)
  dist, h, v = dist or 6000, h or 2500, v or 1500
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local hits = {}
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 then
      pcall(function()
        if fname(readInteger(o + 4)):find("^TnLocationOffsetCameraBehavior_") and readFloat(o + 0x34) == 800 then
          local oi = (readInteger(o + 0x24) >> 4) - 1
          local pn = fname(readInteger(readInteger(go + oi * 4) + 4))
          if pn == "OverTheShoulder_STRATEGY" or (pn:find("^HmCameraStrategy_") and readFloat(o + 0x58) == 800) then
            hits[#hits + 1] = o
          end
        end
      end)
    end
  end
  for _, o in ipairs(hits) do
    writeFloat(o + 0x34, dist)
    if readFloat(o + 0x58) == 800 then writeFloat(o + 0x58, dist) end
    writeFloat(o + 0x2C, h); writeFloat(o + 0x30, v)
  end
  return string.format("giant camera (orbit %g, side %g, up %g) on %d camera behaviours; transform to apply", dist, h, v, #hits)
end

-- ---------------------------------------------------------------------------------------------
-- Omega's own weapons (session 7). Give the weapon in game with `giveweapon TransContent.TnWeaponOmegaRocketArm`
-- (F3 in mods\boss_maps), then run omega_weapons(). It edits the shared data objects (not the weapon instance):
--   OMEGA_Arm_AI_WEPDATA  +0x28 WeaponFireTypes (byte array) -> 1 (projectile), +0x34 WeaponProjectiles -> TnProjectileOmegaHomingRocket,
--                         +0x160 OwnerAnimations -> Bazooka_SP_WEPDATA's animator
--   OMEGA_HomingRocket_AI_PROJDATA +0x44 Damage, +0x48 DamageRadius;  ..._PROJMESH +0x7C DrawScale
-- Offsets were found live, and the field order matches upk_props.py dumps. Do NOT write the weapon instance +0x294/+0x298
-- (that is an array count/capacity, not ammo; writing 999 crashed the game).
local function find_named(name)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and fname(readInteger(o + 4)) == name then return o end
  end
end

function omega_weapons(damage, radius, scale)
  damage, radius, scale = damage or 150, radius or 1500, scale or 3   -- scale 10 = rocket collides with cover early
  local ra, sp = find_named("OMEGA_Arm_AI_WEPDATA"), find_named("Bazooka_SP_WEPDATA")
  local rocket = find_named("TnProjectileOmegaHomingRocket")
  local pd, pm = find_named("OMEGA_HomingRocket_AI_PROJDATA"), find_named("OMEGA_HomingRocket_AI_PROJMESH")
  if not (ra and rocket and pd and pm) then return "Omega weapon data not loaded (is D5_OME_BASE_m loaded?)" end
  local f, a = readInteger(ra + 0x28), readInteger(ra + 0x34)
  for i = 0, readInteger(ra + 0x2C) - 1 do writeBytes(f + i, 1) end
  for i = 0, readInteger(ra + 0x38) - 1 do writeInteger(a + i * 4, rocket) end
  if sp then
    writeInteger(ra + 0x160, readInteger(sp + 0x160))
    -- Megatron's bazooka (aims, HUD, ammo) fires Omega's homing rocket: confirmed in game, homes onto marked enemies
    local b = readInteger(sp + 0x34)
    for i = 0, readInteger(sp + 0x38) - 1 do writeInteger(b + i * 4, rocket) end
  end
  -- the rocket arm has no WeaponMeshTemplate (+0x28C) and never fires; borrow the shooting-frenzy arm's mesh
  local dr, df = find_named("Default__TnWeaponOmegaRocketArm"), find_named("Default__TnWeaponOmegaShootingFrenzyArm")
  if dr and df then writeInteger(dr + 0x28C, readInteger(df + 0x28C)) end
  writeFloat(pd + 0x44, damage); writeFloat(pd + 0x48, radius); writeFloat(pm + 0x7C, scale)
  return string.format("rocket arm -> TnProjectileOmegaHomingRocket x%d, damage %g radius %g scale %g, animator %s",
    readInteger(ra + 0x38), damage, radius, scale, sp and "copied" or "not found")
end

-- Move the weapon attach sockets to Omega's left missile arm (his reload/aim/recoil animations use the left arm).
-- SkeletalMeshSocket (FName = 4 bytes here): +0x28 SocketName, +0x2C BoneName, +0x30 RelativeLocation, +0x3C RelativeRotation.
-- WeaponSocket_Primary/_R are on R_Arm03_Elbow_XB; MissileSocket_Left_01..04 are on L_Weapon01_Wpn_XW. Only Primary is moved (the gun
-- fires from the left, confirmed); WeaponSocket_R (-2700,0,0 on the right elbow) is the tractor-grab hold point. Re-equip the gun afterwards.
function omega_left_arm_sockets()
  local skel = find_named("RB_OmegaSupreme_SKEL")
  if not skel then return "RB_OmegaSupreme_SKEL not loaded" end
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local skidx
  for i = 0, gc - 1 do if readInteger(go + i * 4) == skel then skidx = i break end end
  local socks = {}
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and (readInteger(o + 0x24) >> 4) - 1 == skidx then socks[fname(readInteger(o + 0x28))] = o end
  end
  local src = socks["MissileSocket_Left_01"]
  if not src then return "MissileSocket_Left_01 not found" end
  local done = {}
  for _, nm in ipairs({"WeaponSocket_Primary"}) do   -- WeaponSocket_R stays: it is the real grab hold point
    local d = socks[nm]
    if d then
      writeInteger(d + 0x2C, readInteger(src + 0x2C))
      writeFloat(d + 0x30, 0); writeFloat(d + 0x34, 0); writeFloat(d + 0x38, -800)   -- centre of the four missile sockets
      for k = 0x3C, 0x44, 4 do writeInteger(d + k, readInteger(src + k)) end
      done[#done + 1] = nm
    end
  end
  return "moved to L_Weapon01_Wpn_XW: " .. table.concat(done, ", ") .. " (re-equip the gun)"
end

-- Refill the player's bazooka: weapon instance +0x2A0 clip, +0x2A4 reserve (confirmed live).
-- Player pawn = TnPlayerController (class id 3460) +0x1C8; weapon owner at weapon +0x68.
function omega_ammo(clip, reserve)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local pawn
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and cid(o) == 3460 then pawn = readInteger(o + 0x1C8) end
  end
  local n = 0
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and readInteger(o + 0x68) == pawn and fname(readInteger(o + 4)):find("^TnWeaponBazooka_") then
      writeInteger(o + 0x2A0, clip or 5); writeInteger(o + 0x2A4, reserve or 99); n = n + 1
    end
  end
  return "refilled " .. n .. " bazooka(s)"
end

-- Omega's boss attacks on the player's melee (session 7). The grafted melee set (ROBODEF +0x104, Megatron's HeavyMediumMeleeSet)
-- holds Attacks at +0x48 (TArray), 0x50 bytes per entry, AnimName FName at +0x10 (+0x00 MeleeType, +0x20 Damage).
-- The animation is looked up by name on Omega's AnimSet, so renaming plays his own boss animations.
-- Entries: 4/7/8 Melee_Mace (normal melee), 6 Melee_Ground (air/stomp), 9 Melee_GunButt_H2H (combo). The set is shared with every user of it.
-- Confirmed: Melee_AOE = ground attack with a big shock wave. Melee_ClawCrush_Omega needs a grabbed enemy (do not use for jump).
-- Confirmed: ground melee = Melee_RocketBarrage (its TnAnimNotify_SpawnProjectile events launch TnProjectileMangaRocketOmegaSupreme from
-- RocketBarrageSocket, 8 x 3 rockets, homing, they kill). The jump/stomp slot (6) cuts the animation when Omega lands, so the barrage's
-- first spawn at 2.1 s never fires there. Normal melee played Melee_AOE in full (shock wave + damage).
local OMEGA_MELEE = { [4] = "Melee_RocketBarrage", [7] = "Melee_RocketBarrage", [8] = "Melee_RocketBarrage", [6] = "Melee_AOE", [9] = "Melee_ShootingFrenzy" }

local fname_ids = {}
local function fname_id(str)
  -- FName ids come from Omega's AnimSequence objects (their SequenceName holds the name we need); cached per session
  if fname_ids[str] then return fname_ids[str] end
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and fname(readInteger(o + 4)):find("^AnimSequence_") then
      for k = 0x28, 0x60, 4 do
        local v = readInteger(o + k)
        if v and v > 0 and fname(v) == str then fname_ids[str] = v; return v end
      end
    end
  end
end

function omega_melee(map)
  map = map or OMEGA_MELEE
  local r = find_named("OmegaSupreme_ROBODEF")
  local ms = r and readInteger(r + 0x104)
  if not ms or ms == 0 then return "melee set not grafted yet (run boss_swap_hook and respawn first)" end
  local arr, n = readInteger(ms + 0x48), readInteger(ms + 0x4C)
  local out = {}
  for i, anim in pairs(map) do
    local id = fname_id(anim)
    if id and i < n then writeInteger(arr + i * 0x50 + 0x10, id); out[#out + 1] = i .. "=" .. anim end
  end
  return "melee -> " .. table.concat(out, " ")
end

-- Barrage rockets aim at the player's target (session 7). TnProjectileMangaRocket target picker = TWFC.exe+0x239DA0
-- (EAX = owner pawn + 0x10, returns target + 0x10 or 0x10 = none). If the owner's controller is a TnAiController it returns
-- controller+0x2A4 (the AI's enemy); otherwise it picks from all player pawns + the campaign squad (= our teammates).
-- The cave: if the owner's controller is the player controller (class id 3460), return [[controller+0x568 TnPlayerTargeting]+0x40]
-- (the enemy under the aim), else none (rocket flies straight). Original bytes: 83 EC 18 53 56.
function omega_barrage_targeting()
  -- (Re)install. Fallback list omega_rt_fb: +0 count, +4 round-robin index, +8.. up to 8 pawns (filled by omega_autotarget()).
  -- No aimed enemy -> next pawn from the list (spreads a volley); empty list -> no target (rocket flies straight).
  if not OMEGA_RT_FB then
    if not autoAssemble("alloc(omega_rt_fb,64)" .. string.char(10) .. "registersymbol(omega_rt_fb)" .. string.char(10)) then return "alloc failed" end
    OMEGA_RT_FB = getAddress("omega_rt_fb")
    writeInteger(OMEGA_RT_FB, 0); writeInteger(OMEGA_RT_FB + 4, 0)
  end
  local f = OMEGA_RT_FB
  local ok, err = autoAssemble(string.format([[
alloc(mrt_cave2,256)
registersymbol(mrt_cave2)
label(mrt_orig)
label(mrt_fb)
label(mrt_none)
label(mrt_idx)
mrt_cave2:
  cmp eax,10
  je mrt_orig
  mov ecx,[eax+1C4]
  test ecx,ecx
  je mrt_orig
  mov edx,[ecx+14]
  and edx,FFF
  cmp edx,D85
  jne mrt_orig
  mov edx,[ecx+568]
  test edx,edx
  je mrt_fb
  mov eax,[edx+40]
  test eax,eax
  je mrt_fb
  add eax,10
  ret
mrt_fb:
  mov ecx,[%X]
  test ecx,ecx
  je mrt_none
  mov eax,[%X]
  inc eax
  cmp eax,ecx
  jb mrt_idx
  xor eax,eax
mrt_idx:
  mov [%X],eax
  mov eax,[%X+eax*4]
  test eax,eax
  je mrt_none
  add eax,10
  ret
mrt_none:
  mov eax,10
  ret
mrt_orig:
  sub esp,18
  push ebx
  push esi
  jmp 10B39DA5
10B39DA0:
  jmp mrt_cave2
]], f, f + 4, f + 4, f + 8))
  return ok and "barrage/missile rockets: aimed enemy, else the auto-target list" or ("failed: " .. tostring(err))
end

-- Auto-target list for rockets fired without a marked enemy: up to 4 nearest living Autobots (ROBODEF name contains "Aut") in front of
-- Omega within 30000 units. Candidate pawns are rescanned every 3 s; distances every 0.25 s. Pawn: +0x628 ROBODEF, +0x35C Health (float).
function omega_autotarget(maxn, range)
  maxn, range = maxn or 4, range or 30000
  if not OMEGA_RT_FB then return "run omega_barrage_targeting() first" end
  if OMEGA_AT_TIMER then OMEGA_AT_TIMER.destroy() end
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local pc
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and cid(o) == 3460 then pc = o end
  end
  local cands, t_scan = {}, -10
  OMEGA_AT_TIMER = createTimer(nil, false)
  OMEGA_AT_TIMER.Interval = 250
  OMEGA_AT_TIMER.OnTimer = function()
    if os.clock() - t_scan > 3 then
      t_scan, cands = os.clock(), {}
      go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
      for i = 0, gc - 1 do
        local o = readInteger(go + i * 4)
        if o and o ~= 0 and fname(readInteger(o + 4)):find("^TnAiPawn_") then
          local rd = readInteger(o + 0x628)
          if rd and rd ~= 0 and fname(readInteger(rd + 4)):find("Aut") then cands[#cands + 1] = o end
        end
      end
    end
    local pawn = readInteger(pc + 0x1C8)
    if not pawn or pawn == 0 then writeInteger(OMEGA_RT_FB, 0); return end
    local px, py = readFloat(pawn + 0xD8), readFloat(pawn + 0xDC)
    local yaw = (readInteger(pc + 0xE8) & 0xFFFF) * math.pi / 32768
    local fx, fy = math.cos(yaw), math.sin(yaw)
    local list = {}
    for _, o in ipairs(cands) do
      local hp = readFloat(o + 0x35C)
      if hp and hp > 0 then
        local dx, dy = readFloat(o + 0xD8) - px, readFloat(o + 0xDC) - py
        local dist = math.sqrt(dx * dx + dy * dy)
        if dist > 1 and dist < range and (dx * fx + dy * fy) / dist > 0.3 then list[#list + 1] = { o, dist } end
      end
    end
    table.sort(list, function(x, y) return x[2] < y[2] end)
    local n = math.min(#list, maxn)
    for i = 1, n do writeInteger(OMEGA_RT_FB + 4 + i * 4, list[i][1]) end
    writeInteger(OMEGA_RT_FB, n)
  end
  OMEGA_AT_TIMER.Enabled = true
  return "auto-target on (nearest " .. maxn .. " Autobots in front)"
end

-- Missile sockets (MissileSocket_Left_01..04, rotation pitch +0x3C) point down in the D3B launcher animations: flip them up for those
-- melee modes, restore the boss angle (49152) otherwise.
function omega_missile_sockets(up)
  local skel = find_named("RB_OmegaSupreme_SKEL")
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local skidx
  for i = 0, gc - 1 do if readInteger(go + i * 4) == skel then skidx = i break end end
  local n = 0
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and (readInteger(o + 0x24) >> 4) - 1 == skidx and fname(readInteger(o + 0x28)):find("^MissileSocket_Left_0") then
      writeInteger(o + 0x3C, up and 16384 or 49152); n = n + 1
    end
  end
  return n
end

function omega_barrage_targeting_off()
  writeBytes(0x10B39DA0, 0x83, 0xEC, 0x18, 0x53, 0x56)
  return "barrage targeting restored to the original"
end

-- Melee mode toggle (session 7): the player only ever uses melee entry 4 (ground and air), so a Cheat Engine hotkey switches
-- that entry between the rocket barrage and the AOE ground slam. One beep = barrage, two beeps = AOE. Default key T (unbound in game).
-- Melee modes cycled by the hotkey (beeps = mode number). "missile" = Omega_2nd_Missile_Shoot from the D3B set (his real arm missile
-- launcher, TnProjectileOmegaHomingRocketVignette from MissileSocket_Left_01..04); it needs the VIG_D4_ROBO_ANIM set (omega_add_d3b_anims).
-- Confirmed: both launcher animations hit with the auto-target list + sockets flipped up. Per-mode damage/count tuning (user design:
-- long range = few heavy accurate missiles, short range = many lighter ones) is still to do.
OMEGA_MELEE_MODES = { { name = "barrage", anim = "Melee_RocketBarrage" },
                      { name = "missile", anim = "Big_Omega_Missile_4thEnc" },      -- long range
                      { name = "missile_short", anim = "Omega_2nd_Missile_Shoot" },  -- short range
                      { name = "aoe", anim = "Melee_AOE" } }
function omega_melee_mode_anim()
  for _, m in ipairs(OMEGA_MELEE_MODES) do if m.name == OMEGA_MELEE_MODE then return m.anim end end
  return "Melee_RocketBarrage"
end

function omega_melee_hotkey(vk)
  vk = vk or VK_T
  if OMEGA_MELEE_HK then OMEGA_MELEE_HK.destroy() end
  OMEGA_MELEE_MODE = "barrage"
  omega_melee({ [4] = "Melee_RocketBarrage", [7] = "Melee_AOE", [8] = "Melee_AOE", [6] = "Melee_AOE" })
  OMEGA_MELEE_HK = createHotkey(function()
    if getForegroundProcess() ~= getOpenedProcessID() then return end   -- CE hotkeys are global; ignore typing elsewhere
    local idx = 1
    for i, m in ipairs(OMEGA_MELEE_MODES) do if m.name == OMEGA_MELEE_MODE then idx = i end end
    idx = idx % #OMEGA_MELEE_MODES + 1
    OMEGA_MELEE_MODE = OMEGA_MELEE_MODES[idx].name
    omega_melee({ [4] = OMEGA_MELEE_MODES[idx].anim })
    omega_missile_sockets(OMEGA_MELEE_MODE:find("^missile") ~= nil)
    for i = 1, idx do beep(); sleep(150) end
  end, vk)
  omega_missile_sockets(false)
  return "melee = barrage; T cycles barrage (1 beep) -> long-range missiles (2) -> short-range missiles (3) -> AOE (4)"
end

-- One-call setup. Before the respawn: omega_before_respawn() (swap hook + weapon data, since weapons copy their settings when created).
-- After the respawn as Omega: omega_after_respawn() (camera, smaller collision, left-arm sockets, barrage targeting, melee hotkey T).
function omega_before_respawn()
  return boss_swap_hook("Tank", "OmegaSupreme_ROBODEF", "Megatron_ROBODEF") .. "\n" .. omega_weapons()
end

function omega_after_respawn()
  local out = { boss_camera_giant(), omega_left_arm_sockets(), omega_barrage_targeting(), omega_melee_hotkey(), omega_real_grab(), omega_add_d3b_anims(), omega_autotarget() }
  -- collision radius 800 (Omega's 3100 blocks him everywhere; a transform may reset it)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and cid(o) == 3460 then
      local cyl = readInteger(readInteger(o + 0x1C8) + 0x15C)
      if cyl and cyl ~= 0 then writeFloat(cyl + 0x1B0, 800); out[#out + 1] = "collision radius 800" end
    end
  end
  return table.concat(out, "\n")
end

-- Fake tractor beam (session 7). The real grab is an AI action (TnAiActionGrab -> TnGrabber state machine, needs the AI world state's
-- locked-on target, LTCG register calling conventions), so this is driven from Cheat Engine instead:
--   hold the key while aiming at a marked enemy -> held in front of Omega's claw, its Health set to 1, melee = Melee_ClawCrush_Omega
--   (melee while holding = claw crush, whose hit kills it); release -> thrown along the aim (falling physics + velocity), melee restored.
-- Offsets (from the property objects: dword +0x30 >> 20): Actor Location +0xD8, Velocity +0xF0, Physics byte +0x64 (1 walk, 2 fall),
-- Pawn Health +0x35C. Controller: +0xE4 pitch, +0xE8 yaw (65536 = 360 deg), +0x568 TnPlayerTargeting (+0x40 = aimed enemy).
function omega_tractor(vk, hold_dist, hold_up, throw_speed)
  vk, hold_dist, hold_up, throw_speed = vk or VK_X, hold_dist or 3500, hold_up or 1200, throw_speed or 6000
  if OMEGA_TR_TIMER then OMEGA_TR_TIMER.destroy() end
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local pc
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and cid(o) == 3460 then pc = o end
  end
  if not pc then return "player controller not found" end
  local held, held_name, crush_at
  local function s16(v) v = v & 0xFFFF; return v >= 0x8000 and v - 0x10000 or v end
  local function restore_melee()
    omega_melee({ [4] = omega_melee_mode_anim() })
  end
  OMEGA_TR_TIMER = createTimer(nil, false)
  OMEGA_TR_TIMER.Interval = 25
  OMEGA_TR_TIMER.OnTimer = function()
    local pawn = readInteger(pc + 0x1C8)
    if not pawn or pawn == 0 then held = nil; return end
    if held and fname(readInteger(held + 4) or 0) ~= held_name then held = nil; restore_melee() end
    local yaw = s16(readInteger(pc + 0xE8)) * math.pi / 32768
    local pitch = s16(readInteger(pc + 0xE4)) * math.pi / 32768
    if isKeyPressed(vk) then
      if not held then
        local tg = readInteger(pc + 0x568)
        local e = tg and tg ~= 0 and readInteger(tg + 0x40)
        if e and e ~= 0 then
          held, held_name = e, fname(readInteger(e + 4))
          if readInteger(e + 0x35C) > 1 then writeInteger(e + 0x35C, 1) end
          -- play the tractor-grab animation: melee entry 4 -> Melee_TractorGrab and press the melee key (Q) once;
          -- ~0.5 s later melee becomes the claw crush (pressing Q while holding = crush)
          omega_melee({ [4] = "Melee_TractorGrab" })
          doKeyPress(VK_Q)
          crush_at = os.clock() + 0.5
          beep()
        end
      end
      if held and crush_at and os.clock() >= crush_at then omega_melee({ [4] = "Melee_ClawCrush_Omega" }); crush_at = nil end
      if held then
        local px, py, pz = readFloat(pawn + 0xD8), readFloat(pawn + 0xDC), readFloat(pawn + 0xE0)
        writeFloat(held + 0xD8, px + math.cos(yaw) * hold_dist)
        writeFloat(held + 0xDC, py + math.sin(yaw) * hold_dist)
        writeFloat(held + 0xE0, pz + hold_up)
        writeFloat(held + 0xF0, 0); writeFloat(held + 0xF4, 0); writeFloat(held + 0xF8, 0)
      end
    elseif held then
      local c = math.cos(pitch)
      writeBytes(held + 0x64, 2)                                         -- PHYS_Falling so the velocity is kept
      writeFloat(held + 0xF0, c * math.cos(yaw) * throw_speed)
      writeFloat(held + 0xF4, c * math.sin(yaw) * throw_speed)
      writeFloat(held + 0xF8, math.sin(pitch) * throw_speed + 1500)
      held, crush_at = nil, nil
      restore_melee()
    end
  end
  OMEGA_TR_TIMER.Enabled = true
  return "tractor beam: hold the key on a marked enemy (melee = claw crush), release to throw"
end

function omega_tractor_off()
  if OMEGA_TR_TIMER then OMEGA_TR_TIMER.destroy(); OMEGA_TR_TIMER = nil end
  return "tractor beam off"
end

-- REAL tractor-beam grab (session 7, confirmed). F11 (CheatManager.Teleport, native wrapper at TWFC.exe+0x13E8670) is replaced by a cave
-- that runs on the game thread:
--   press 1: victim = aimed enemy ([[PC+0x568]+0x40]); victim ref = {victim, 0, victim+0x10 vtbl[1](L"TnServiceProvider")};
--            grabber = [pawn+0x568] (TnGrabber), attacker interface = grabber+0x38; call iface slot +0x14 (0x10AB5390,
--            thiscall, ret 8)(&GrabDescription, &victim ref). GrabDescription = Omega's TnAiActionGrabBlueprint +0xD8
--            (LiftTime 1, LiftAccel 2000, LiftSpeed 1000, Rot 180, TractorAccel 5000, TractorSpeed 3000, AttachSocket WeaponSocket_R,
--            DetachDamage 1000 TnDamageTypeOmegaTractorBeamGrab, DetachImpulse 1e6). States (grabber+0xB4): 0 Inactive .. 4 Holding.
--   press 2: if Holding -> detach with damage 0x10AB66C0 (thiscall this=grabber+0x10, ret 4), then slot +0x18 (0x10AB5470, GotoState Inactive).
-- While holding, a watcher sets melee entry 4 to Melee_ClawCrush_Omega (the crush animation does not cancel the grab).
local function find_by_name_pat(pat)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and fname(readInteger(o + 4)):find(pat) then return o end
  end
end

-- Grab speed presets. The cave uses its own copy of the GrabDescription at omega_grab_data+0x40 (0x38 bytes), so the original blueprint
-- (the boss values) stays untouched. Layout: +0 LiftTime, +4 LiftAcceleration, +8 LiftSpeed, +C RotationRate, +14 TractorAcceleration,
-- +18 TractorSpeed, +20 AttachSocket, +24 DetachDamage, +28 DetachDamageType, +2C DetachImpulse.
local GRAB_SPEEDS = {
  slow = nil,                                                         -- the boss fight's original values
  fast = { lift_time = 0.3, lift_accel = 10000, lift_speed = 3000, tractor_accel = 30000, tractor_speed = 12000 },
}
function omega_grab_speed(name)
  if not (OMEGA_GRAB_DATA and OMEGA_GRAB_BP_DESC) then return "run omega_real_grab() first" end
  local copy = OMEGA_GRAB_DATA + 0x40
  copyMemory(OMEGA_GRAB_BP_DESC, 0x38, copy)
  local v = GRAB_SPEEDS[name]
  if v then
    writeFloat(copy + 0x00, v.lift_time); writeFloat(copy + 0x04, v.lift_accel); writeFloat(copy + 0x08, v.lift_speed)
    writeFloat(copy + 0x14, v.tractor_accel); writeFloat(copy + 0x18, v.tractor_speed)
  end
  writeInteger(OMEGA_GRAB_DATA + 4, copy)
  OMEGA_GRAB_SPEED = name
  return string.format("grab speed %s: lift %.1f s, tractor speed %g", name, readFloat(copy), readFloat(copy + 0x18))
end

-- Tractor animation used while the enemy is pulled in (looped per cycle until the grabber is Holding). Tested: A = arm moves, the
-- enemy never reaches the hold point; B = much better; C = B's animation at natural speed with a fast finish.
-- A melee cannot interrupt itself, so the crush waits for the current cycle: shorter cycles = shorter wait.
--   A: Melee_TractorGrab at normal speed (1.7 s cycles)
--   B: Shooting_Tell (the AI grab's aiming animation, 5.37 s) at 2x speed (~2.7 s cycles)
-- Rate = AnimSequence +0x40 RateScale (+0x38 SequenceLength, +0x28 SequenceName).
local function anim_seq(name)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and fname(readInteger(o + 4)):find("^AnimSequence_") and fname(readInteger(o + 0x28)) == name then return o end
  end
end

--   C: Shooting_Tell at normal speed; when the enemy reaches the claw the rest of the cycle plays at 16x, then the crush.
--   D (default): Shooting_Tell plays to the aiming pose, crawls at 0.05x while pulling (no blend back to idle), fast finish, crush.
function omega_grab_style(which)
  local st = anim_seq("Shooting_Tell")
  local rate = (which == "B") and 2.0 or 1.0
  if st then writeFloat(st + 0x40, rate) end
  local anim = (which == "A") and "Melee_TractorGrab" or "Shooting_Tell"
  local seq = anim_seq(anim)
  local len = seq and (readFloat(seq + 0x38) / readFloat(seq + 0x40)) or 1.7
  OMEGA_GRAB_TRACTOR = { anim = anim, len = len + 0.15, seq = (which == "C") and st or nil, rate = rate, fast = (which == "C") and 16.0 or nil }
  if which == "D" then
    OMEGA_GRAB_TRACTOR = { anim = "Shooting_Tell", seq = st, full = st and readFloat(st + 0x38) or 5.37,
                           hold_at = OMEGA_GRAB_HOLD_AT or 2.0, hold_rate = 0.05, fast = 16.0 }
  end
  return string.format("grab style %s: %s, %.2f s per cycle", which, anim, len)
end

function omega_real_grab()
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local pc
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and cid(o) == 3460 then pc = o end
  end
  local bp
  for i = 0, gc - 1 do       -- Omega's grab blueprint: TnAiActionGrabBlueprint under OmegaSupreme_AiActionSet
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and fname(readInteger(o + 4)):find("^TnAiActionGrabBlueprint_") and readFloat(o + 0xEC) == 5000 and readFloat(o + 0xFC) == 1000 then bp = o end
  end
  if not (pc and bp) then return "player controller or Omega grab blueprint not found" end
  if not OMEGA_GRAB_DATA then
    local ok = autoAssemble([[
alloc(omega_grab_data,256)
registersymbol(omega_grab_data)
]])
    if not ok then return "alloc failed" end
    OMEGA_GRAB_DATA = getAddress("omega_grab_data")
  end
  local d = OMEGA_GRAB_DATA
  -- +0 pc, +4 desc, +8 holding, +C calls, +10 victim ref (3 dwords), +1C L"TnServiceProvider"
  writeInteger(d, pc); writeInteger(d + 8, 0); writeInteger(d + 0xC, 0)
  OMEGA_GRAB_BP_DESC = bp + 0xD8
  omega_grab_speed(OMEGA_GRAB_SPEED or "fast")
  writeString(d + 0x1C, "TnServiceProvider", true); writeBytes(d + 0x1C + 34, 0, 0)
  local ok, err = autoAssemble(string.format([[
alloc(omega_grab_cave,512)
registersymbol(omega_grab_cave)
label(g_ret)
label(g_release)
label(g_cancel)
omega_grab_cave:
  pushad
  mov esi,[%X]
  test esi,esi
  je g_ret
  mov eax,[esi+1C8]
  test eax,eax
  je g_ret
  mov ebx,[eax+568]
  test ebx,ebx
  je g_ret
  add ebx,38
  cmp dword ptr [%X],0
  jne g_release
  mov edx,[esi+568]
  test edx,edx
  je g_ret
  mov edi,[edx+40]
  test edi,edi
  je g_ret
  mov [%X],edi
  mov dword ptr [%X],0
  lea ecx,[edi+10]
  mov eax,[ecx]
  push %X
  call dword ptr [eax+4]
  test eax,eax
  je g_ret
  mov [%X],eax
  mov ecx,ebx
  push %X
  push dword ptr [%X]
  mov eax,[ecx]
  call dword ptr [eax+14]
  mov dword ptr [%X],1
  inc dword ptr [%X]
  jmp g_ret
g_release:
  cmp byte ptr [ebx+7C],4
  jne g_cancel
  lea ecx,[ebx-28]
  push 0
  call 10AB66C0
g_cancel:
  mov ecx,ebx
  mov eax,[ecx]
  call dword ptr [eax+18]
  mov dword ptr [%X],0
  inc dword ptr [%X]
g_ret:
  popad
  ret 8
117E8670:
  jmp omega_grab_cave
]], d, d + 8, d + 0x10, d + 0x14, d + 0x1C, d + 0x18, d + 0x10, d + 4, d + 8, d + 0xC, d + 8, d + 0xC))
  if not ok then return "grab cave failed: " .. tostring(err) end
  -- Automatic sequence (user design): F11 grab -> Shooting_Tell (the AI grab's tractor/aim animation) while lifting/tractoring;
  -- grabber reaches Holding (state 4) -> Melee_ClawCrush_Omega; RELEASE_TIME s later -> throw (the cave's release path).
  -- Animations go through melee entry 4 + a simulated Q press; the throw through a simulated F11 press (runs on the game thread).
  -- RELEASE_TIME 5.3 = TnAiActionThrowEnemyBlueprint.ReleaseTime (crush animation is 7.37 s; the throw happens near the end).
  local RELEASE_TIME = OMEGA_GRAB_RELEASE_TIME or 5.3
  local function mode_anim() return omega_melee_mode_anim() end
  local phase, t_crush, t_next, t_start, grab_pawn, t_hold = "idle", 0, 0, 0, nil, nil
  if not OMEGA_GRAB_TRACTOR then omega_grab_style("D") end
  if OMEGA_GRAB_WATCH then OMEGA_GRAB_WATCH.destroy() end
  OMEGA_GRAB_WATCH = createTimer(nil, false)
  OMEGA_GRAB_WATCH.Interval = 50
  OMEGA_GRAB_WATCH.OnTimer = function()
    local holding = readInteger(d + 8) == 1
    local pawn = readInteger(pc + 0x1C8)
    local grabber = pawn and pawn ~= 0 and readInteger(pawn + 0x568)
    local gstate = grabber and grabber ~= 0 and readBytes(grabber + 0xB4, 1) or 0
    local function reset_anim()
      local trr = OMEGA_GRAB_TRACTOR
      if trr and trr.seq then writeFloat(trr.seq + 0x40, 1.0) end
      local r = find_named("OmegaSupreme_ROBODEF")
      if r then writeFloat(readInteger(readInteger(r + 0x104) + 0x48) + 4 * 0x50 + 0x18, 0.35) end
      omega_melee({ [4] = mode_anim() })
    end
    if not holding then
      if phase ~= "idle" then phase = "idle"; reset_anim() end
      return
    end
    -- self-heal: the grab ended without our throw (respawn, new pawn, grabber back to Inactive) -> clear the flag
    if phase ~= "idle" and (pawn ~= grab_pawn or (gstate == 0 and os.clock() - t_start > 1.5)) then
      writeInteger(d + 8, 0); phase = "idle"; reset_anim(); return
    end
    local tr = OMEGA_GRAB_TRACTOR or { anim = "Melee_TractorGrab", len = 1.7 }
    if tr.hold_at then
      -- style D: the tractor animation never ends while pulling. Play to the aiming pose (hold_at s), then crawl at hold_rate x;
      -- when the enemy reaches the claw finish the rest at fast x and start the crush. Entry 4 BlendOut (+0x18) shortened meanwhile.
      local e4 = readInteger(readInteger(find_named("OmegaSupreme_ROBODEF") + 0x104) + 0x48) + 4 * 0x50
      local now = os.clock()
      if phase == "idle" then
        phase, grab_pawn, t_start = "pull", pawn, now
        writeFloat(tr.seq + 0x40, 1.0)
        omega_melee({ [4] = tr.anim })
        writeFloat(e4 + 0x18, 0.05)
        doKeyPress(VK_Q)
        t_hold = nil
      elseif phase == "pull" then
        if not t_hold and now - t_start >= tr.hold_at then
          writeFloat(tr.seq + 0x40, tr.hold_rate); t_hold = now
        end
        if gstate == 4 then
          local pos = t_hold and (tr.hold_at + (now - t_hold) * tr.hold_rate) or (now - t_start)
          writeFloat(tr.seq + 0x40, tr.fast)
          t_next = now + math.max(0, tr.full - pos) / tr.fast + 0.05
          phase = "finish"
        end
      elseif phase == "finish" and now >= t_next then
        writeFloat(tr.seq + 0x40, 1.0)
        writeFloat(e4 + 0x18, 0.35)
        phase = "crush"
        omega_melee({ [4] = "Melee_ClawCrush_Omega" })
        doKeyPress(VK_Q)
        t_crush = now
      elseif phase == "crush" and now - t_crush >= RELEASE_TIME then
        phase = "thrown"
        doKeyPress(VK_F11)
      end
      return
    end
    if phase == "idle" then                                   -- F11 grab just happened
      phase, grab_pawn, t_start = "tractor", pawn, os.clock()
      omega_melee({ [4] = tr.anim })
      doKeyPress(VK_Q)
      t_next = os.clock() + tr.len
    elseif phase == "tractor" and gstate == 4 and tr.seq and tr.fast then
      -- style C: enemy arrived -> finish the current cycle at tr.fast x speed (RateScale is applied every frame), then crush
      local remaining = math.max(0, t_next - 0.15 - os.clock())
      writeFloat(tr.seq + 0x40, tr.fast)
      t_next = os.clock() + remaining / tr.fast + 0.15
      phase = "finish"
    elseif (phase == "tractor" or phase == "finish") and os.clock() >= t_next then   -- a melee cannot interrupt itself
      if gstate == 4 then                                     -- enemy is in the claw -> crush
        phase = "crush"
        if tr.seq then writeFloat(tr.seq + 0x40, tr.rate) end
        omega_melee({ [4] = "Melee_ClawCrush_Omega" })
        doKeyPress(VK_Q)
        t_crush = os.clock()
      else                                                    -- still pulling -> another tractor cycle
        doKeyPress(VK_Q)
        t_next = os.clock() + tr.len
      end
    elseif phase == "crush" and os.clock() - t_crush >= RELEASE_TIME then
      phase = "thrown"
      doKeyPress(VK_F11)                                       -- cave release: detach with 1000 damage + impulse
    end
  end
  OMEGA_GRAB_WATCH.Enabled = true
  return "real grab on F11: grab -> tractor animation -> claw crush -> automatic throw (1000 damage)"
end

function omega_real_grab_off()
  writeBytes(0x117E8670, 0x57, 0x8B, 0xF9, 0xE8, 0xB8)   -- original: push edi / mov edi,ecx / call ...
  if OMEGA_GRAB_WATCH then OMEGA_GRAB_WATCH.destroy(); OMEGA_GRAB_WATCH = nil end
  return "F11 is Teleport again"
end

-- Preview Omega's D3B (pre-boss Iacon encounter) animations. They live in TR_OmegaSupreme_ANM_p.VIG_D4_ROBO_ANIM, loaded by the DLC pack
-- (D3B_IAC_MiddleDome_VIG_m). The set is added to Omega's mesh AnimSets so melee (entry 4) can play them by name; P (CE hotkey, game
-- in front) steps to the next animation, plays it and speaks its name.
local D3B_ANIMS = { "Omega_Reach", "Omega_KongReach", "Omega_KongPull", "Omega_RipOff_Idle", "Omega_KongFall", "Omega_Smash_Roar",
  "Omega_Smash_Roar_Pillar", "Big_Omega_Smash_4thEnc", "Big_Omega_Missile_4thEnc", "Omega_2nd_Missile_Shoot", "D4_4thEncounter_ROBO",
  "Land_Bridge", "Fall_Bridge" }

local function find_animset(name, skel_pat)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and fname(readInteger(o + 4)) == name then
      local oi = (readInteger(o + 0x24) >> 4) - 1
      local outer = oi >= 0 and readInteger(go + oi * 4)
      if outer and fname(readInteger(outer + 4)) == "TR_OmegaSupreme_ANM_p" then return o end
    end
  end
end

-- add an AnimSet to Omega's mesh AnimSets (TArray found by scanning the mesh for an array of AnimSets that holds OmegaSupreme_ROBO)
function omega_add_animset(set)
  local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
  local pawn
  for i = 0, gc - 1 do
    local o = readInteger(go + i * 4)
    if o and o ~= 0 and cid(o) == 3460 then pawn = readInteger(o + 0x1C8) end
  end
  local mesh = readInteger(pawn + 0x3F4)
  for k = 0x60, 0x400, 4 do
    local data, n, max = readInteger(mesh + k), readInteger(mesh + k + 4), readInteger(mesh + k + 8)
    if data and n and max and n > 0 and n <= 32 and max >= n and max <= 64 then
      local first = readInteger(data)
      local ok, nm = pcall(function() return fname(readInteger(first + 4)) end)
      if ok and nm == "OmegaSupreme_ROBO" then
        for j = 0, n - 1 do if readInteger(data + j * 4) == set then return "already in AnimSets (+" .. string.format("%X", k) .. ")" end end
        if max > n then
          writeInteger(data + n * 4, set); writeInteger(mesh + k + 4, n + 1)
          return string.format("added to AnimSets at mesh+%X (now %d)", k, n + 1)
        end
        return string.format("AnimSets at mesh+%X is full (%d/%d)", k, n, max)
      end
    end
  end
  return "AnimSets array not found"
end

function omega_add_d3b_anims()
  local set = find_animset("VIG_D4_ROBO_ANIM")
  if not set then return "D3B animations not loaded (DLC pack: D3B_IAC_MiddleDome_VIG_m)" end
  -- their spawn notifies fire the set-piece rocket (no targeting): switch to the barrage rocket (homes via the targeting cave).
  -- TnAnimNotify_SpawnProjectile live layout: ProjectileClass at +0x34.
  local vig, manga = find_named("TnProjectileOmegaHomingRocketVignette"), find_named("TnProjectileMangaRocketOmegaSupreme")
  local switched = 0
  if vig and manga then
    local go, gc = readInteger(GOBJECTS), readInteger(GOBJECTS + 4)
    for i = 0, gc - 1 do
      local o = readInteger(go + i * 4)
      if o and o ~= 0 and readInteger(o + 0x34) == vig and fname(readInteger(o + 4)):find("^TnAnimNotify_SpawnProjectile") then
        writeInteger(o + 0x34, manga); switched = switched + 1
      end
    end
  end
  return "D3B animations: " .. omega_add_animset(set) .. "; " .. switched .. " missile spawns -> homing barrage rocket"
end

function omega_preview_d3b()
  local set = find_animset("VIG_D4_ROBO_ANIM")
  if not set then return "VIG_D4_ROBO_ANIM not loaded (DLC pack needs D3B_IAC_MiddleDome_VIG_m)" end
  local r = omega_add_animset(set)
  OMEGA_PREVIEW_I = 0
  if OMEGA_PREVIEW_HK then OMEGA_PREVIEW_HK.destroy() end
  OMEGA_PREVIEW_HK = createHotkey(function()
    if getForegroundProcess() ~= getOpenedProcessID() then return end
    OMEGA_PREVIEW_I = OMEGA_PREVIEW_I % #D3B_ANIMS + 1
    local a = D3B_ANIMS[OMEGA_PREVIEW_I]
    omega_melee({ [4] = a })
    doKeyPress(VK_Q)
    if speak then pcall(speak, a:gsub("_", " ")) end
  end, VK_P)
  return r .. "; P = play next D3B animation (" .. #D3B_ANIMS .. "), its name is spoken"
end
