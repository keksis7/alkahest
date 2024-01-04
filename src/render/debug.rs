use std::sync::Arc;

use super::bytecode::externs::TfxShaderStage;
use super::{color::Color, shader, ConstantBuffer, DeviceContextSwapchain};
use crate::ecs::transform::Transform;
use crate::types::AABB;
use anyhow::Context;
use genmesh::generators::IndexedPolygon;
use genmesh::generators::SharedVertex;
use genmesh::Triangulate;
use glam::Vec4;
use glam::{Mat4, Quat, Vec3};
use itertools::Itertools;
use windows::Win32::Graphics::{
    Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_LINELIST, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST},
    Direct3D11::{
        ID3D11Buffer, ID3D11InputLayout, ID3D11PixelShader, ID3D11VertexShader,
        D3D11_BIND_INDEX_BUFFER, D3D11_BIND_VERTEX_BUFFER, D3D11_BUFFER_DESC,
        D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_SUBRESOURCE_DATA,
        D3D11_USAGE_IMMUTABLE,
    },
    Dxgi::Common::{DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R32G32B32A32_FLOAT},
};

#[derive(Clone)]
pub enum DebugShape {
    Cube {
        cube: AABB,
        rotation: Quat,
        sides: bool,
    },
    Sphere {
        center: Vec3,
        radius: f32,
    },
    Line {
        start: Vec3,
        end: Vec3,
        dotted: bool,
        dot_scale: f32,
    },
    Custom {
        transform: Transform,
        shape: CustomDebugShape,
        sides: bool,
    },
}

#[derive(Default)]
pub struct DebugShapes {
    shapes: Vec<(DebugShape, Color)>,
    labels: Vec<(String, Vec3, egui::Align2, Color)>,
}

impl DebugShapes {
    pub fn cube_extents<C: Into<Color>>(
        &mut self,
        center: Vec3,
        extents: Vec3,
        rotation: Quat,
        color: C,
        sides: bool,
    ) {
        let min = center - extents;
        let max = center + extents;

        self.shapes.push((
            DebugShape::Cube {
                cube: AABB {
                    min: min.into(),
                    max: max.into(),
                },
                rotation,
                sides,
            },
            color.into(),
        ))
    }

    pub fn cube_aabb<C: Into<Color>>(&mut self, aabb: AABB, rotation: Quat, color: C, sides: bool) {
        self.shapes.push((
            DebugShape::Cube {
                cube: aabb,
                rotation,
                sides,
            },
            color.into(),
        ))
    }

    pub fn sphere<C: Into<Color>>(&mut self, center: Vec3, radius: f32, color: C) {
        self.shapes
            .push((DebugShape::Sphere { center, radius }, color.into()))
    }

    pub fn line<C: Into<Color>>(&mut self, start: Vec3, end: Vec3, color: C) {
        self.shapes.push((
            DebugShape::Line {
                start,
                end,
                dotted: false,
                dot_scale: 1.0,
            },
            color.into(),
        ))
    }

    pub fn line_dotted<C: Into<Color>>(
        &mut self,
        start: Vec3,
        end: Vec3,
        color: C,
        dot_scale: f32,
    ) {
        self.shapes.push((
            DebugShape::Line {
                start,
                end,
                dotted: true,
                dot_scale,
            },
            color.into(),
        ))
    }

    pub fn line_orientation<C: Into<Color>>(
        &mut self,
        point: Vec3,
        orientation: Quat,
        length: f32,
        color: C,
    ) {
        self.line(point, point + orientation * Vec3::X * length, color.into())
    }

    pub fn cross<C: Into<Color> + Copy>(&mut self, point: Vec3, length: f32, color: C) {
        let color = color.into();
        let half_length = length / 2.0;
        self.line(
            point - Vec3::X * half_length,
            point + Vec3::X * half_length,
            color,
        );
        self.line(
            point - Vec3::Y * half_length,
            point + Vec3::Y * half_length,
            color,
        );
        self.line(
            point - Vec3::Z * half_length,
            point + Vec3::Z * half_length,
            color,
        );
    }

    pub fn custom_shape<C: Into<Color>>(
        &mut self,
        transform: Transform,
        shape: CustomDebugShape,
        color: C,
        sides: bool,
    ) {
        self.shapes.push((
            DebugShape::Custom {
                transform,
                shape,
                sides,
            },
            color.into(),
        ))
    }

    pub fn text<C: Into<Color>>(
        &mut self,
        text: String,
        point: Vec3,
        anchor: egui::Align2,
        color: C,
    ) {
        self.labels.push((text, point, anchor, color.into()))
    }

    // /// See `FpsCamera::calculate_frustum_corners` for index layout
    // /// Silently returns if corners.len() != 8
    // pub fn frustum_corners<C: Into<Color> + Copy>(&mut self, corners: &[Vec3], color: C) {
    //     if corners.len() != 8 {
    //         return;
    //     }

    //     for (p1, p2) in [
    //         (0_usize, 4_usize), // bottom left
    //         (1, 5),             // bottom right
    //         (2, 6),             // top left
    //         (3, 7),             // top right
    //         (4, 5),             // far bottom
    //         (6, 7),             // far top
    //         (4, 6),             // far left
    //         (5, 7),             // far right
    //         (0, 1),             // near bottom
    //         (2, 3),             // near top
    //         (0, 2),             // near left
    //         (1, 3),             // near right
    //     ] {
    //         self.line(corners[p1], corners[p2], color);
    //     }
    // }

    /// Returns the drawlist. The internal list is cleared after this call
    pub fn shape_list(&mut self) -> Vec<(DebugShape, Color)> {
        let v = self.shapes.clone();
        self.shapes.clear();

        v
    }

    pub fn label_list(&mut self) -> Vec<(String, Vec3, egui::Align2, Color)> {
        let v = self.labels.clone();
        self.labels.clear();

        v
    }
}

// TODO(cohae): We can improve performance by instancing each type of shape using instance buffers
pub struct DebugShapeRenderer {
    dcs: Arc<DeviceContextSwapchain>,
    scope: ConstantBuffer<ScopeAlkDebugShape>,
    scope_line: ConstantBuffer<ScopeAlkDebugShapeLine>,
    vshader: ID3D11VertexShader,
    vshader_line: ID3D11VertexShader,
    pshader: ID3D11PixelShader,
    pshader_line: ID3D11PixelShader,
    pshader_line_dotted: ID3D11PixelShader,

    input_layout: ID3D11InputLayout,

    vb_sphere: ID3D11Buffer,
    ib_sphere: ID3D11Buffer,
    sphere_index_count: u32,

    vb_cube: ID3D11Buffer,
    ib_cube: ID3D11Buffer,
    ib_cube_sides: ID3D11Buffer,
    cube_outline_index_count: u32,
    cube_index_count: u32,
}

impl DebugShapeRenderer {
    pub fn new(dcs: Arc<DeviceContextSwapchain>) -> anyhow::Result<Self> {
        let data_vscube = shader::compile_hlsl(
            include_str!("../../assets/shaders/debug.hlsl"),
            "VShader",
            "vs_5_0",
        )
        .unwrap();
        let (vshader, _) = shader::load_vshader(&dcs, &data_vscube)?;
        let data_vsline = shader::compile_hlsl(
            include_str!("../../assets/shaders/debug_line.hlsl"),
            "VShader",
            "vs_5_0",
        )
        .unwrap();
        let (vshader_line, _) = shader::load_vshader(&dcs, &data_vsline)?;

        let input_layout = unsafe {
            dcs.device.CreateInputLayout(
                &[D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("POSITION"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 0,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                }],
                &data_vscube,
            )
        }
        .unwrap();

        let data = shader::compile_hlsl(
            include_str!("../../assets/shaders/debug.hlsl"),
            "PShader",
            "ps_5_0",
        )
        .unwrap();
        let (pshader, _) = shader::load_pshader(&dcs, &data)?;

        let data = shader::compile_hlsl(
            include_str!("../../assets/shaders/debug_line.hlsl"),
            "PShader",
            "ps_5_0",
        )
        .unwrap();
        let (pshader_line, _) = shader::load_pshader(&dcs, &data)?;

        let data = shader::compile_hlsl(
            include_str!("../../assets/shaders/debug_line.hlsl"),
            "PShaderDotted",
            "ps_5_0",
        )
        .unwrap();
        let (pshader_line_dotted, _) = shader::load_pshader(&dcs, &data)?;

        let mesh_sphere = genmesh::generators::SphereUv::new(16, 16);
        let vertices: Vec<[f32; 4]> = mesh_sphere
            .shared_vertex_iter()
            .map(|v| {
                let v = <[f32; 3]>::from(v.pos);
                [v[0], v[1], v[2], 1.0]
            })
            .collect();

        let mut indices = vec![];
        for i in mesh_sphere.indexed_polygon_iter().triangulate() {
            indices.extend_from_slice(&[i.x as u16, i.y as u16, i.z as u16]);
        }

        let ib_sphere = unsafe {
            dcs.device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (indices.len() * 2) as _,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_INDEX_BUFFER,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: indices.as_ptr() as _,
                        ..Default::default()
                    }),
                )
                .context("Failed to create index buffer")?
        };

        let vb_sphere = unsafe {
            dcs.device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (vertices.len() * 16) as _,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_VERTEX_BUFFER,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: vertices.as_ptr() as _,
                        ..Default::default()
                    }),
                )
                .context("Failed to create combined vertex buffer")?
        };

        let sphere_index_count = indices.len() as _;

        let mesh = genmesh::generators::Cube::new();
        let vertices: Vec<[f32; 4]> = mesh
            .shared_vertex_iter()
            .map(|v| {
                let v = <[f32; 3]>::from(v.pos);
                [v[0], v[1], v[2], 1.0]
            })
            .collect();
        let mut indices = vec![];
        let mut indices_outline = vec![];
        for i in mesh.indexed_polygon_iter().triangulate() {
            indices.extend_from_slice(&[i.x as u16, i.y as u16, i.z as u16]);
        }

        for i in mesh.indexed_polygon_iter() {
            indices_outline.extend_from_slice(&[
                i.x as u16, i.y as u16, i.y as u16, i.z as u16, i.z as u16, i.w as u16, i.w as u16,
                i.x as u16,
            ]);
        }

        let ib_cube = unsafe {
            dcs.device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (indices_outline.len() * 2) as _,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_INDEX_BUFFER,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: indices_outline.as_ptr() as _,
                        ..Default::default()
                    }),
                )
                .context("Failed to create index buffer")?
        };

        let ib_cube_sides = unsafe {
            dcs.device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (indices.len() * 2) as _,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_INDEX_BUFFER,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: indices.as_ptr() as _,
                        ..Default::default()
                    }),
                )
                .context("Failed to create index buffer")?
        };

        let vb_cube = unsafe {
            dcs.device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (vertices.len() * 16) as _,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_VERTEX_BUFFER,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: vertices.as_ptr() as _,
                        ..Default::default()
                    }),
                )
                .context("Failed to create combined vertex buffer")?
        };

        Ok(Self {
            scope: ConstantBuffer::create(dcs.clone(), None)?,
            scope_line: ConstantBuffer::create(dcs.clone(), None)?,
            dcs,
            vshader,
            vshader_line,
            pshader,
            pshader_line,
            pshader_line_dotted,
            input_layout,
            vb_sphere,
            ib_sphere,
            sphere_index_count,
            vb_cube,
            ib_cube,
            ib_cube_sides,
            cube_index_count: indices.len() as _,
            cube_outline_index_count: indices_outline.len() as _,
        })
    }

    pub fn draw_all(&self, shapes: &mut DebugShapes) {
        for (shape, color) in shapes.shape_list() {
            match shape {
                DebugShape::Custom {
                    transform,
                    shape,
                    sides,
                } => {
                    self.scope
                        .write(&ScopeAlkDebugShape {
                            model: transform.to_mat4(),
                            color,
                        })
                        .unwrap();

                    self.scope.bind(10, TfxShaderStage::Vertex);
                    self.scope.bind(10, TfxShaderStage::Pixel);

                    unsafe {
                        self.dcs.context().IASetInputLayout(&self.input_layout);
                        self.dcs.context().VSSetShader(&self.vshader, None);
                        self.dcs.context().PSSetShader(&self.pshader, None);

                        self.dcs.context().IASetVertexBuffers(
                            0,
                            1,
                            Some([Some(shape.vb.clone())].as_ptr()),
                            Some([16].as_ptr()),
                            Some(&0),
                        );

                        self.dcs.context().IASetIndexBuffer(
                            Some(&shape.ib),
                            DXGI_FORMAT_R16_UINT,
                            0,
                        );

                        self.dcs
                            .context()
                            .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_LINELIST);

                        self.dcs
                            .context()
                            .DrawIndexed(shape.outline_index_count as _, 0, 0);
                    }

                    if sides {
                        self.scope
                            .write(&ScopeAlkDebugShape {
                                model: transform.to_mat4(),
                                color: Color(color.0.truncate().extend(0.35)),
                            })
                            .unwrap();

                        unsafe {
                            self.dcs.context().IASetVertexBuffers(
                                0,
                                1,
                                Some([Some(shape.vb.clone())].as_ptr()),
                                Some([16].as_ptr()),
                                Some(&0),
                            );

                            self.dcs.context().IASetIndexBuffer(
                                Some(&shape.ib_sides),
                                DXGI_FORMAT_R16_UINT,
                                0,
                            );

                            self.dcs
                                .context()
                                .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

                            self.dcs.context().DrawIndexed(shape.index_count, 0, 0);
                        }
                    }
                }
                DebugShape::Cube {
                    cube,
                    rotation,
                    sides,
                } => {
                    self.scope
                        .write(&ScopeAlkDebugShape {
                            model: Mat4::from_scale_rotation_translation(
                                cube.extents(),
                                rotation,
                                cube.center(),
                            ),
                            color,
                        })
                        .unwrap();

                    self.scope.bind(10, TfxShaderStage::Vertex);
                    self.scope.bind(10, TfxShaderStage::Pixel);

                    unsafe {
                        self.dcs.context().IASetInputLayout(&self.input_layout);
                        self.dcs.context().VSSetShader(&self.vshader, None);
                        self.dcs.context().PSSetShader(&self.pshader, None);

                        self.dcs.context().IASetVertexBuffers(
                            0,
                            1,
                            Some([Some(self.vb_cube.clone())].as_ptr()),
                            Some([16].as_ptr()),
                            Some(&0),
                        );

                        self.dcs.context().IASetIndexBuffer(
                            Some(&self.ib_cube),
                            DXGI_FORMAT_R16_UINT,
                            0,
                        );

                        self.dcs
                            .context()
                            .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_LINELIST);

                        self.dcs
                            .context()
                            .DrawIndexed(self.cube_outline_index_count as _, 0, 0);
                    }

                    if sides {
                        self.scope
                            .write(&ScopeAlkDebugShape {
                                model: Mat4::from_scale_rotation_translation(
                                    cube.extents(),
                                    rotation,
                                    cube.center(),
                                ),
                                color: Color(color.0.truncate().extend(0.25)),
                            })
                            .unwrap();

                        unsafe {
                            self.dcs.context().IASetVertexBuffers(
                                0,
                                1,
                                Some([Some(self.vb_cube.clone())].as_ptr()),
                                Some([16].as_ptr()),
                                Some(&0),
                            );

                            self.dcs.context().IASetIndexBuffer(
                                Some(&self.ib_cube_sides),
                                DXGI_FORMAT_R16_UINT,
                                0,
                            );

                            self.dcs
                                .context()
                                .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

                            self.dcs.context().DrawIndexed(self.cube_index_count, 0, 0);
                        }
                    }
                }
                DebugShape::Sphere { center, radius } => {
                    self.scope
                        .write(&ScopeAlkDebugShape {
                            model: Mat4::from_scale_rotation_translation(
                                Vec3::splat(radius),
                                Quat::IDENTITY,
                                center,
                            ),
                            color,
                        })
                        .unwrap();

                    self.scope.bind(10, TfxShaderStage::Vertex);
                    self.scope.bind(10, TfxShaderStage::Pixel);

                    unsafe {
                        self.dcs.context().IASetInputLayout(&self.input_layout);
                        self.dcs.context().VSSetShader(&self.vshader, None);
                        self.dcs.context().PSSetShader(&self.pshader, None);

                        self.dcs.context().IASetVertexBuffers(
                            0,
                            1,
                            Some([Some(self.vb_sphere.clone())].as_ptr()),
                            Some([16].as_ptr()),
                            Some(&0),
                        );

                        self.dcs.context().IASetIndexBuffer(
                            Some(&self.ib_sphere),
                            DXGI_FORMAT_R16_UINT,
                            0,
                        );

                        self.dcs
                            .context()
                            .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

                        self.dcs
                            .context()
                            .DrawIndexed(self.sphere_index_count, 0, 0);
                    }
                }
                DebugShape::Line {
                    start,
                    end,
                    dotted,
                    dot_scale,
                } => {
                    self.scope_line
                        .write(&ScopeAlkDebugShapeLine {
                            start: start.extend(1.0),
                            end: end.extend(1.0),
                            color,
                            dot_scale,
                        })
                        .unwrap();

                    self.scope_line.bind(10, TfxShaderStage::Vertex);
                    self.scope_line.bind(10, TfxShaderStage::Pixel);

                    unsafe {
                        self.dcs.context().VSSetShader(&self.vshader_line, None);
                        if dotted {
                            self.dcs
                                .context()
                                .PSSetShader(&self.pshader_line_dotted, None);
                        } else {
                            self.dcs.context().PSSetShader(&self.pshader_line, None);
                        }

                        self.dcs
                            .context()
                            .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_LINELIST);

                        self.dcs.context().Draw(2, 0);
                    }
                }
            }
        }
    }
}

#[repr(C)]
pub struct ScopeAlkDebugShape {
    pub model: Mat4,
    pub color: Color,
}

#[repr(C)]
pub struct ScopeAlkDebugShapeLine {
    pub start: Vec4,
    pub end: Vec4,
    pub color: Color,
    pub dot_scale: f32,
}

#[derive(Clone)]
pub struct CustomDebugShape {
    vb: ID3D11Buffer,
    ib: ID3D11Buffer,
    ib_sides: ID3D11Buffer,
    outline_index_count: u32,
    index_count: u32,
}

impl CustomDebugShape {
    pub fn new(
        dcs: &DeviceContextSwapchain,
        vertices: &[Vec4],
        indices: &[u16],
    ) -> anyhow::Result<CustomDebugShape> {
        let indices_outline = remove_diagonals_linegulate(vertices, indices);

        let ib = unsafe {
            dcs.device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (indices_outline.len() * 2) as _,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_INDEX_BUFFER,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: indices_outline.as_ptr() as _,
                        ..Default::default()
                    }),
                )
                .context("Failed to create index buffer")?
        };

        let ib_sides = unsafe {
            dcs.device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (indices.len() * 2) as _,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_INDEX_BUFFER,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: indices.as_ptr() as _,
                        ..Default::default()
                    }),
                )
                .context("Failed to create index buffer")?
        };

        let vb = unsafe {
            dcs.device
                .CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (vertices.len() * 16) as _,
                        Usage: D3D11_USAGE_IMMUTABLE,
                        BindFlags: D3D11_BIND_VERTEX_BUFFER,
                        ..Default::default()
                    },
                    Some(&D3D11_SUBRESOURCE_DATA {
                        pSysMem: vertices.as_ptr() as _,
                        ..Default::default()
                    }),
                )
                .context("Failed to create combined vertex buffer")?
        };

        Ok(Self {
            vb,
            ib,
            ib_sides,
            outline_index_count: indices_outline.len() as _,
            index_count: indices.len() as _,
        })
    }

    pub fn from_havok_shape(
        dcs: &DeviceContextSwapchain,
        shape: &destiny_havok::shape_collection::Shape,
    ) -> anyhow::Result<Self> {
        let vertices_vec4 = shape.vertices.iter().map(|v| v.extend(1.0)).collect_vec();
        Self::new(dcs, &vertices_vec4, &shape.indices)
    }
}

// Input: triangulated mesh, output: line list with diagonal edges removed
pub fn remove_diagonals_linegulate(vertices: &[Vec4], indices: &[u16]) -> Vec<u16> {
    let mut indices_outline = vec![];
    for i in indices.chunks_exact(3) {
        let i0 = i[0];
        let i1 = i[1];
        let i2 = i[2];

        let v0 = vertices[i0 as usize];
        let v1 = vertices[i1 as usize];
        let v2 = vertices[i2 as usize];

        //         0
        //         |\ edge_a
        //  edge c | \
        //         2--1
        //           edge_b
        let edge_a = (v0 - v1).length();
        let edge_b = (v1 - v2).length();
        let edge_c = (v2 - v0).length();

        if edge_a > edge_b && edge_a > edge_c {
            indices_outline.extend_from_slice(&[i0, i2, i2, i1]);
        } else if edge_b > edge_a && edge_b > edge_c {
            indices_outline.extend_from_slice(&[i0, i1, i0, i2]);
        } else if edge_c > edge_a && edge_c > edge_b {
            indices_outline.extend_from_slice(&[i0, i1, i1, i2]);
        } else {
            indices_outline.extend_from_slice(&[i0, i1, i1, i2, i2, i0])
        }
    }

    indices_outline
}
