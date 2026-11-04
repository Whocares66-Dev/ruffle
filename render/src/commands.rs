use crate::backend::ShapeHandle;
use crate::bitmap::{BitmapHandle, PixelRegion, PixelSnapping};
use crate::matrix::Matrix;
use crate::pixel_bender::PixelBenderShaderHandle;
use crate::transform::Transform;
use swf::{BlendMode, Color};

pub trait CommandHandler {
    fn render_bitmap(
        &mut self,
        bitmap: BitmapHandle,
        transform: Transform,
        smoothing: bool,
        pixel_snapping: PixelSnapping,
        region: PixelRegion,
    );
    fn render_stage3d(&mut self, bitmap: BitmapHandle, transform: Transform);
    fn render_shape(&mut self, shape: ShapeHandle, transform: Transform);
    fn render_alpha_mask(&mut self, maskee_commands: CommandList, mask_commands: CommandList);
    fn draw_rect(&mut self, color: Color, matrix: Matrix);
    fn draw_line(&mut self, color: Color, matrix: Matrix);
    fn draw_line_rect(&mut self, color: Color, matrix: Matrix);
    fn push_mask(&mut self);
    fn activate_mask(&mut self);
    fn deactivate_mask(&mut self);
    fn pop_mask(&mut self);

    fn blend(&mut self, commands: CommandList, blend_mode: RenderBlendMode);
}

/// Holds either a normal BlendMode, or the shader for BlendMode.SHADER.
///
/// We cannot store the `PixelBenderShaderHandle` directly in `ExtendedBlendMode`,
/// since we need to remember the shader even if the blend mode is changed
/// to something else (so that the shader will still be used if we switch back)
#[derive(Debug, Clone)]
pub enum RenderBlendMode {
    Builtin(BlendMode),
    Shader(PixelBenderShaderHandle),
}

#[derive(Debug, Default, Clone)]
pub struct CommandList {
    pub commands: Vec<Command>,

    /// The number of mask regions in the process of being drawn.
    /// This is used to discard drawing commands of nested maskers, which Flash does not support.
    maskers_in_progress: u32,
}

impl CommandList {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fingerprint of what the list draws, for a host that keeps a picture while it does not change: shapes and
    /// bitmaps by their handles' identity (the host keeps the last list's handles alive, so an address is not reused
    /// while it is compared), transforms by their numbers. `None` when something in it cannot be told (a perspective
    /// projection, a 3D stage, a pixel bender blend): then it is taken to have changed.
    pub fn fingerprint(&self) -> Option<u64> {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash_into(&mut hasher).then(|| hasher.finish())
    }

    fn hash_into(&self, h: &mut std::collections::hash_map::DefaultHasher) -> bool {
        use std::hash::Hash;
        fn matrix(h: &mut std::collections::hash_map::DefaultHasher, m: &Matrix) {
            for v in [m.a, m.b, m.c, m.d] {
                v.to_bits().hash(h);
            }
            m.tx.get().hash(h);
            m.ty.get().hash(h);
        }
        fn transform(h: &mut std::collections::hash_map::DefaultHasher, t: &Transform) -> bool {
            matrix(h, &t.matrix);
            t.tz.to_bits().hash(h);
            let c = &t.color_transform;
            for v in [
                c.r_multiply.get(),
                c.g_multiply.get(),
                c.b_multiply.get(),
                c.a_multiply.get(),
            ] {
                v.hash(h);
            }
            for v in [c.r_add, c.g_add, c.b_add, c.a_add] {
                v.hash(h);
            }
            t.perspective_projection.is_none()
        }
        fn color(h: &mut std::collections::hash_map::DefaultHasher, c: &Color) {
            [c.r, c.g, c.b, c.a].hash(h);
        }
        self.commands.len().hash(h);
        for command in &self.commands {
            std::mem::discriminant(command).hash(h);
            let known = match command {
                Command::RenderBitmap {
                    bitmap,
                    transform: t,
                    smoothing,
                    pixel_snapping,
                    region,
                } => {
                    (std::sync::Arc::as_ptr(&bitmap.0) as *const () as usize).hash(h);
                    smoothing.hash(h);
                    std::mem::discriminant(pixel_snapping).hash(h);
                    [region.x_min, region.y_min, region.x_max, region.y_max].hash(h);
                    transform(h, t)
                }
                Command::RenderStage3D { .. } => false,
                Command::RenderShape {
                    shape,
                    transform: t,
                } => {
                    (std::sync::Arc::as_ptr(&shape.0) as *const () as usize).hash(h);
                    transform(h, t)
                }
                Command::RenderAlphaMask {
                    maskee_commands,
                    mask_commands,
                } => maskee_commands.hash_into(h) && mask_commands.hash_into(h),
                Command::DrawRect {
                    color: c,
                    matrix: m,
                }
                | Command::DrawLine {
                    color: c,
                    matrix: m,
                }
                | Command::DrawLineRect {
                    color: c,
                    matrix: m,
                } => {
                    color(h, c);
                    matrix(h, m);
                    true
                }
                Command::PushMask
                | Command::ActivateMask
                | Command::DeactivateMask
                | Command::PopMask => true,
                Command::Blend(list, mode) => match mode {
                    RenderBlendMode::Builtin(mode) => {
                        std::mem::discriminant(mode).hash(h);
                        list.hash_into(h)
                    }
                    RenderBlendMode::Shader(_) => false,
                },
            };
            if !known {
                return false;
            }
        }
        true
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn execute(self, handler: &mut impl CommandHandler) {
        for command in self.commands {
            match command {
                Command::RenderBitmap {
                    bitmap,
                    transform,
                    smoothing,
                    pixel_snapping,
                    region,
                } => handler.render_bitmap(bitmap, transform, smoothing, pixel_snapping, region),
                Command::RenderShape { shape, transform } => handler.render_shape(shape, transform),
                Command::RenderStage3D { bitmap, transform } => {
                    handler.render_stage3d(bitmap, transform)
                }
                Command::DrawRect { color, matrix } => handler.draw_rect(color, matrix),
                Command::DrawLine { color, matrix } => handler.draw_line(color, matrix),
                Command::DrawLineRect { color, matrix } => handler.draw_line_rect(color, matrix),
                Command::PushMask => handler.push_mask(),
                Command::ActivateMask => handler.activate_mask(),
                Command::DeactivateMask => handler.deactivate_mask(),
                Command::PopMask => handler.pop_mask(),
                Command::Blend(commands, blend_mode) => handler.blend(commands, blend_mode),
                Command::RenderAlphaMask {
                    maskee_commands,
                    mask_commands,
                } => handler.render_alpha_mask(maskee_commands, mask_commands),
            }
        }
    }

    pub fn drawing_mask(&self) -> bool {
        self.maskers_in_progress > 0
    }
}

impl CommandHandler for CommandList {
    #[inline]
    fn render_bitmap(
        &mut self,
        bitmap: BitmapHandle,
        transform: Transform,
        smoothing: bool,
        pixel_snapping: PixelSnapping,
        region: PixelRegion,
    ) {
        if self.maskers_in_progress <= 1 {
            self.commands.push(Command::RenderBitmap {
                bitmap,
                transform,
                smoothing,
                pixel_snapping,
                region,
            });
        }
    }

    #[inline]
    fn render_stage3d(&mut self, bitmap: BitmapHandle, transform: Transform) {
        if self.maskers_in_progress <= 1 {
            self.commands
                .push(Command::RenderStage3D { bitmap, transform });
        }
    }

    #[inline]
    fn render_shape(&mut self, shape: ShapeHandle, transform: Transform) {
        if self.maskers_in_progress <= 1 {
            self.commands
                .push(Command::RenderShape { shape, transform });
        }
    }

    fn render_alpha_mask(&mut self, maskee_commands: CommandList, mask_commands: CommandList) {
        if self.maskers_in_progress <= 1 {
            self.commands.push(Command::RenderAlphaMask {
                maskee_commands,
                mask_commands,
            });
        }
    }

    #[inline]
    fn draw_rect(&mut self, color: Color, matrix: Matrix) {
        if self.maskers_in_progress <= 1 {
            self.commands.push(Command::DrawRect { color, matrix });
        }
    }

    #[inline]
    fn draw_line(&mut self, color: Color, matrix: Matrix) {
        if self.maskers_in_progress <= 1 {
            self.commands.push(Command::DrawLine { color, matrix });
        }
    }

    #[inline]
    fn draw_line_rect(&mut self, color: Color, matrix: Matrix) {
        if self.maskers_in_progress <= 1 {
            self.commands.push(Command::DrawLineRect { color, matrix });
        }
    }

    #[inline]
    fn push_mask(&mut self) {
        if self.maskers_in_progress == 0 {
            self.commands.push(Command::PushMask);
        }
        self.maskers_in_progress += 1;
    }

    #[inline]
    fn activate_mask(&mut self) {
        self.maskers_in_progress -= 1;
        if self.maskers_in_progress == 0 {
            self.commands.push(Command::ActivateMask);
        }
    }

    #[inline]
    fn deactivate_mask(&mut self) {
        if self.maskers_in_progress == 0 {
            self.commands.push(Command::DeactivateMask);
        }
        self.maskers_in_progress += 1;
    }

    #[inline]
    fn pop_mask(&mut self) {
        self.maskers_in_progress -= 1;
        if self.maskers_in_progress == 0 {
            self.commands.push(Command::PopMask);
        }
    }

    #[inline]
    fn blend(&mut self, commands: CommandList, blend_mode: RenderBlendMode) {
        if self.maskers_in_progress <= 1 {
            self.commands.push(Command::Blend(commands, blend_mode));
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    RenderBitmap {
        bitmap: BitmapHandle,
        transform: Transform,
        smoothing: bool,
        pixel_snapping: PixelSnapping,
        region: PixelRegion,
    },
    RenderStage3D {
        bitmap: BitmapHandle,
        transform: Transform,
    },
    RenderShape {
        shape: ShapeHandle,
        transform: Transform,
    },
    RenderAlphaMask {
        maskee_commands: CommandList,
        mask_commands: CommandList,
    },
    DrawRect {
        color: Color,
        matrix: Matrix,
    },
    DrawLine {
        color: Color,
        matrix: Matrix,
    },
    DrawLineRect {
        color: Color,
        matrix: Matrix,
    },
    PushMask,
    ActivateMask,
    DeactivateMask,
    PopMask,
    Blend(CommandList, RenderBlendMode),
}
