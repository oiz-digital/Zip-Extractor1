//! EVM execution stack (1024 slots, 256-bit wide).

use zbx_types::U256;
use thiserror::Error;

/// Maximum EVM stack depth.
pub const MAX_STACK_SIZE: usize = 1024;

#[derive(Debug, Error)]
pub enum StackError {
    #[error("stack overflow (> {MAX_STACK_SIZE} items)")]
    Overflow,
    #[error("stack underflow")]
    Underflow,
}

/// The EVM stack.
pub struct Stack {
    data: Vec<U256>,
}

impl Stack {
    pub fn new() -> Self {
        Self { data: Vec::with_capacity(64) }
    }

    pub fn push(&mut self, v: U256) -> Result<(), StackError> {
        if self.data.len() >= MAX_STACK_SIZE {
            return Err(StackError::Overflow);
        }
        self.data.push(v);
        Ok(())
    }

    pub fn pop(&mut self) -> Result<U256, StackError> {
        self.data.pop().ok_or(StackError::Underflow)
    }

    pub fn peek(&self, n: usize) -> Result<&U256, StackError> {
        let len = self.data.len();
        if n >= len { return Err(StackError::Underflow); }
        Ok(&self.data[len - 1 - n])
    }

    pub fn peek_mut(&mut self, n: usize) -> Result<&mut U256, StackError> {
        let len = self.data.len();
        if n >= len { return Err(StackError::Underflow); }
        Ok(&mut self.data[len - 1 - n])
    }

    pub fn dup(&mut self, n: usize) -> Result<(), StackError> {
        if n == 0 || n > 16 { return Err(StackError::Underflow); }
        let v = *self.peek(n - 1)?;
        self.push(v)
    }

    pub fn swap(&mut self, n: usize) -> Result<(), StackError> {
        if n == 0 || n > 16 { return Err(StackError::Underflow); }
        let len = self.data.len();
        if n >= len { return Err(StackError::Underflow); }
        self.data.swap(len - 1, len - 1 - n);
        Ok(())
    }

    pub fn len(&self) -> usize { self.data.len() }
    pub fn is_empty(&self) -> bool { self.data.is_empty() }

    pub fn clear(&mut self) { self.data.clear(); }
}

impl Default for Stack {
    fn default() -> Self { Self::new() }
}