use crate::entity::ai::control::Control;
use crate::entity::mob::Mob;

#[derive(Default)]
pub struct JumpControl {
    jump: bool,
}

impl Control for JumpControl {}

impl JumpControl {
    pub const fn jump(&mut self) {
        self.jump = true;
    }

    /// Vanilla `RabbitJumpControl.wantJump`: a jump was asked for and not yet ticked.
    #[must_use]
    pub const fn wants_jump(&self) -> bool {
        self.jump
    }

    pub fn tick(&mut self, mob: &dyn Mob) {
        mob.tick_jump_control(self.jump);
        self.jump = false;
    }
}
