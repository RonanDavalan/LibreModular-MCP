import bpy

tree = bpy.data.node_groups.new("Scatter", "GeometryNodeTree")
for socket in list(tree.interface.items_tree):
    tree.interface.remove(socket)
tree.interface.new_socket("Geometry", in_out="INPUT", socket_type="NodeSocketGeometry")
tree.interface.new_socket("Geometry", in_out="OUTPUT", socket_type="NodeSocketGeometry")
group_in = tree.nodes.new("NodeGroupInput")
group_out = tree.nodes.new("NodeGroupOutput")
tree.links.new(group_in.outputs[0], group_out.inputs[0])
unused = [node for node in tree.nodes if not node.inputs and not node.outputs]
for node in unused:
    tree.nodes.remove(node)
result = [node.name for node in tree.nodes]
