use tracing::instrument;

use crate::pipeline::{PiperError, Value, ValueType};

use super::Function;

#[derive(Clone, Debug)]
pub struct TypeConverterFunction {
    pub to: ValueType,
}

impl Function for TypeConverterFunction {
    fn get_output_type(&self, argument_types: &[ValueType]) -> Result<ValueType, PiperError> {
        if argument_types.len() != 1 {
            return Err(PiperError::ArityError(
                format!("to_{}", self.to),
                argument_types.len(),
            ));
        }
        Ok(self.to)
    }

    #[instrument(level = "trace", skip(self))]
    fn eval(&self, mut arguments: Vec<Value>) -> Value {
        if arguments.len() != 1 {
            return Value::Error(PiperError::InvalidArgumentCount(1, arguments.len()));
        }
        arguments.remove(0).convert_to(self.to)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_conv() {
        use super::*;
        use crate::pipeline::function::Function;
        use crate::pipeline::ValueType;

        let f = TypeConverterFunction { to: ValueType::Int };
        assert!(f.get_output_type(&[ValueType::Int]).is_ok());
        assert!(f.get_output_type(&[]).is_err());
        assert!(f.get_output_type(&[ValueType::Int, ValueType::Int]).is_err());
        assert_eq!(f.eval(vec![Value::Int(1)]), Value::Int(1));
        assert_eq!(f.eval(vec![Value::Double(1.2)]), Value::Int(1));
    }
}