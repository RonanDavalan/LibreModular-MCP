#!/usr/bin/env python3
# Piste A — noyau-orbital — Blender only, fond transparent, non minimaliste.
# Hub central + 2 anneaux-outils + 5 satellites-modules + plateforme + cables + boulons.
import bpy, os, math

TAILLE = int(os.environ.get("TAILLE", "512"))
MONO = os.environ.get("MONO", "")
OUT = os.path.join(os.environ.get("SORTIE", "."), "piste-a-noyau-orbital-" + str(TAILLE) + ("-mono-" + MONO if MONO else "") + ".png")
os.makedirs(os.path.dirname(os.path.abspath(OUT)), exist_ok=True)

# --- nettoyage ---
bpy.ops.object.select_all(action='SELECT')
bpy.ops.object.delete(use_global=False)
for coll in (bpy.data.meshes, bpy.data.curves, bpy.data.materials):
    for x in list(coll):
        try: coll.remove(x)
        except: pass

def mat_std(nom, rgba, metallic=0.0, roughness=0.55):
    m = bpy.data.materials.new(nom)
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = rgba
    try: bsdf.inputs["Metallic"].default_value = metallic
    except: pass
    try: bsdf.inputs["Roughness"].default_value = roughness
    except: pass
    return m

def mat_emit(nom, rgba, force=6.0):
    m = bpy.data.materials.new(nom)
    m.use_nodes = True
    nt = m.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    em = nt.nodes.new("ShaderNodeEmission")
    em.inputs["Color"].default_value = rgba
    em.inputs["Strength"].default_value = force
    nt.links.new(em.outputs[0], out.inputs[0])
    return m

BLEU = (0.11, 0.22, 0.36, 1.0)      # #1f3a5f hub
CUIVRE = (0.76, 0.33, 0.22, 1.0)    # #c4553a
AMBRE = (0.95, 0.70, 0.26, 1.0)     # #f2b344 outil 1
CYAN = (0.16, 0.63, 0.59, 1.0)      # #2aa198 outil 2
CLAIRE = (0.90, 0.89, 0.88, 1.0)
SOMBRE = (0.05, 0.06, 0.09, 1.0)
m_hub = mat_std("Hub", BLEU, 0.85, 0.32)
m_cuivre = mat_std("Cuivre", CUIVRE, 0.6, 0.4)
m_clair = mat_std("Clair", CLAIRE, 0.1, 0.5)
m_sombre = mat_std("Sombre", SOMBRE, 0.3, 0.6)
m_ambre = mat_emit("Ambre", AMBRE, 2.2)
m_cyan = mat_emit("Cyan", CYAN, 2.2)
m_blanc_emit = mat_emit("Blanc", (1, 1, 1, 1), 1.6)

if MONO == "noir":
    _mono = mat_std("Mono", (0.04,0.04,0.05,1.0), 0.2, 0.6)
    m_hub = m_cuivre = m_clair = m_sombre = m_ambre = m_cyan = m_blanc_emit = _mono
if MONO == "blanc":
    _mono = mat_std("Mono", (0.96,0.96,0.95,1.0), 0.0, 0.6)
    m_hub = m_cuivre = m_clair = m_sombre = m_ambre = m_cyan = m_blanc_emit = _mono
def donner(obj, mat):
    if obj.data and hasattr(obj.data, "materials"):
        obj.data.materials.clear()
        obj.data.materials.append(mat)
    return obj

# --- plateforme hexagonale ---
bpy.ops.mesh.primitive_cylinder_add(vertices=6, radius=2.9, depth=0.35, location=(0, 0, -1.55))
plate = donner(bpy.context.active_object, m_sombre)
bpy.ops.mesh.primitive_cylinder_add(vertices=6, radius=2.55, depth=0.12, location=(0, 0, -1.32))
donner(bpy.context.active_object, m_cuivre)
# boulons autour
for i in range(6):
    a = i * math.pi / 3
    bpy.ops.mesh.primitive_cylinder_add(radius=0.11, depth=0.22, location=(2.3*math.cos(a), 2.3*math.sin(a), -1.22))
    donner(bpy.context.active_object, m_clair)

# --- hub central ---
bpy.ops.mesh.primitive_uv_sphere_add(radius=1.15, location=(0, 0, 0.1))
hub = donner(bpy.context.active_object, m_hub)
bpy.ops.mesh.primitive_ico_sphere_add(radius=1.42, location=(0, 0, 0.1))
cage = donner(bpy.context.active_object, m_cuivre)
# cage : mode wire via modificateur
mod = cage.modifiers.new("Cage", 'WIREFRAME')
mod.thickness = 0.035
# noyau interne emissif
bpy.ops.mesh.primitive_uv_sphere_add(radius=0.45, location=(0, 0, 0.1))
donner(bpy.context.active_object, m_blanc_emit)
# deux pastilles outils sur le hub
for dx, mat in ((-0.42, m_ambre), (0.42, m_cyan)):
    bpy.ops.mesh.primitive_uv_sphere_add(radius=0.20, location=(dx, -0.95, 0.35))
    donner(bpy.context.active_object, mat)

# --- deux anneaux-outils ---
bpy.ops.mesh.primitive_torus_add(major_radius=1.75, minor_radius=0.10, location=(0, 0, 0.1), rotation=(math.pi/2, 0, 0.5))
donner(bpy.context.active_object, m_ambre)
bpy.ops.mesh.primitive_torus_add(major_radius=2.05, minor_radius=0.07, location=(0, 0, 0.1), rotation=(0.4, math.pi/2.3, 0))
donner(bpy.context.active_object, m_cyan)
# plots lumineux sur anneaux
for i in range(8):
    a = i * math.pi / 4
    bpy.ops.mesh.primitive_uv_sphere_add(radius=0.07, location=(1.75*math.cos(a+0.5), 1.75*math.sin(a+0.5), 0.1))
    donner(bpy.context.active_object, m_ambre if i % 2 == 0 else m_cyan)

# --- 5 satellites-modules + fenetres + cables ---
sats = [
    (2.55, 0.0, 0.9, m_cuivre, "gimp"),
    (-1.3, 2.25, 1.25, m_clair, "krita"),
    (-1.3, -2.25, 0.6, m_hub, "blender"),
    (1.35, 1.95, -0.35, m_cuivre, "inkscape"),
    (1.35, -1.95, -0.15, m_clair, "office"),
]
for i, (x, y, z, mat, _nom) in enumerate(sats):
    bpy.ops.mesh.primitive_cube_add(size=0.85, location=(x, y, z))
    sat = donner(bpy.context.active_object, mat)
    bev = sat.modifiers.new("Bevel", 'BEVEL')
    bev.width = 0.10
    bev.segments = 2
    # fenetre emissive
    mm = m_ambre if i % 2 == 0 else m_cyan
    bpy.ops.mesh.primitive_cube_add(size=0.30, location=(x, y - 0.44 if y >= 0 else y + 0.44, z + 0.12))
    donner(bpy.context.active_object, mm)
    # petite antenne
    bpy.ops.mesh.primitive_cylinder_add(radius=0.035, depth=0.55, location=(x, y, z + 0.65))
    donner(bpy.context.active_object, m_sombre)
    bpy.ops.mesh.primitive_uv_sphere_add(radius=0.08, location=(x, y, z + 0.95))
    donner(bpy.context.active_object, mm)
    # cable courbe hub -> satellite
    bpy.ops.curve.primitive_bezier_curve_add(location=(0, 0, 0))
    cur = bpy.context.active_object
    sp = cur.data.splines[0]
    p0, p1 = sp.bezier_points[0], sp.bezier_points[1]
    p0.co = (0.3, 0.0, -0.2)
    p1.co = (x * 0.82, y * 0.82, z * 0.6)
    p0.handle_right = (0.8, 0.4, 0.3)
    p1.handle_left = (x * 0.5, y * 0.5, z * 0.9)
    cur.data.bevel_depth = 0.035
    donner(cur, m_sombre)

# --- tuyaux bas + rivets (densite non minimaliste) ---
for i in range(4):
    a = i * math.pi / 2 + 0.3
    bpy.ops.mesh.primitive_cylinder_add(radius=0.09, depth=1.6, location=(1.1*math.cos(a), 1.1*math.sin(a), -1.0), rotation=(math.pi/2, 0, -a))
    donner(bpy.context.active_object, m_cuivre)
for i in range(10):
    a = i * math.pi * 2 / 10
    bpy.ops.mesh.primitive_uv_sphere_add(radius=0.06, location=(2.62*math.cos(a), 2.62*math.sin(a), -1.42))
    donner(bpy.context.active_object, m_clair)

# --- camera + lumieres ---
bpy.ops.object.camera_add(location=(0, -8.8, 3.4),rotation=(1.18,0,0))
cam = bpy.context.active_object
cam.data.lens = 50
bpy.context.scene.camera = cam
bpy.ops.object.light_add(type='SUN', location=(4, -3, 8))
bpy.context.active_object.data.energy = 3.2
bpy.ops.object.light_add(type='AREA', location=(-4, -4, 4))
al = bpy.context.active_object
al.data.energy = 260
al.data.size = 4
bpy.ops.object.light_add(type='AREA', location=(3, 3, 2))
al2 = bpy.context.active_object
al2.data.energy = 120
al2.data.size = 3

# --- rendu transparent ---
sc = bpy.context.scene
sc.render.engine = 'BLENDER_EEVEE_NEXT' if 'BLENDER_EEVEE_NEXT' in dir(bpy.types) else sc.render.engine
try:
    sc.render.engine = 'BLENDER_EEVEE_NEXT'
except:
    try: sc.render.engine = 'BLENDER_EEVEE'
    except: pass
sc.render.film_transparent = True
sc.render.resolution_x = TAILLE
sc.render.resolution_y = TAILLE
sc.render.resolution_percentage = 100
sc.render.image_settings.file_format = 'PNG'
sc.render.image_settings.color_mode = 'RGBA'
sc.render.filepath = OUT
bpy.ops.render.render(write_still=True)
print("OK piste A : " + OUT)
