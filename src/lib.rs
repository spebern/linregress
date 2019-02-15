#![warn(rust_2018_idioms)]
use failure::{bail, err_msg, Error};
use nalgebra::{DMatrix, DVector, RowDVector};
use statrs::distribution::{StudentsT, Univariate};
use std::collections::HashMap;
use std::iter;

/// A builder to create a linear regression model
///
#[derive(Debug)]
pub struct FormulaRegressionBuilder {
    data: Option<HashMap<String, Vec<f64>>>,
    formula: Option<String>,
}
impl Default for FormulaRegressionBuilder {
    fn default() -> Self {
        FormulaRegressionBuilder::new()
    }
}

impl FormulaRegressionBuilder {
    pub fn new() -> Self {
        FormulaRegressionBuilder {
            data: None,
            formula: None,
        }
    }
    pub fn data(mut self, data: &HashMap<String, Vec<f64>>) -> Self {
        self.data = Some(data.to_owned());
        self
    }
    pub fn formula<T: Into<String>>(mut self, formula: T) -> Self {
        self.formula = Some(formula.into());
        self
    }
    pub fn fit(self) -> Result<RegressionModel, Error> {
        let data: Result<_, Error> = self
            .data
            .ok_or_else(|| err_msg("Cannot fit model without data"));
        let formula: Result<_, Error> = self
            .formula
            .ok_or_else(|| err_msg("Cannot fit model without formula"));
        let data = data?;
        let formula = formula?;
        let split_formula: Vec<_> = formula.split('~').collect();
        if split_formula.len() != 2 {
            bail!("Invalid formula. Expected formula of the form 'y ~ x1 + x2'");
        }
        let input = split_formula[0].trim();
        let outputs: Vec<_> = split_formula[1]
            .split('+')
            .map(|x| x.trim())
            .filter(|x| *x != "")
            .collect();
        if outputs.is_empty() {
            bail!("Invalid formula. Expected formula of the form 'y ~ x1 + x2'");
        }
        let input_vector = data
            .get(input)
            .ok_or_else(|| err_msg(format!("{} not found in data", input)))?;
        let input_vector = RowDVector::from_vec(input_vector.to_vec());
        let mut output_matrix = Vec::new();
        // Add column of all ones as the first column of the matrix
        let all_ones_column = iter::repeat(1.).take(input_vector.len());
        output_matrix.extend(all_ones_column);
        // Add each input as a new column of the matrix
        for output in outputs.to_owned() {
            let output_vec = data
                .get(output)
                .ok_or_else(|| err_msg(format!("{} not found in data", output)))?;
            if output_vec.len() != input_vector.len() {
                bail!(format!(
                    "Regressor dimensions for {} do not match regressand dimensions",
                    output
                ));
            }
            output_matrix.extend(output_vec.iter());
        }
        //
        let output_matrix = DMatrix::from_vec(input_vector.len(), outputs.len() + 1, output_matrix);
        let (model_parameter, singular_values, normalized_cov_params) =
            Self::fit_ols_pinv(&input_vector, &output_matrix)?;
        RegressionModel::try_from_regression_parameters(
            &input_vector,
            &output_matrix,
            &model_parameter,
            &singular_values,
            &normalized_cov_params,
            data,
            formula,
        )
    }
    // XXX move to Regression result as with_ols_...
    /// Performs ordinary least squared linear regression using the pseudo inverse method to solve
    /// the linear system.
    ///
    /// Returns a tuple of the form `(model_parameter, singular_values, normalized_cov_params)` as `Result`.
    fn fit_ols_pinv(
        inputs: &RowDVector<f64>,
        outputs: &DMatrix<f64>,
    ) -> Result<(DMatrix<f64>, DVector<f64>, DMatrix<f64>), Error> {
        let singular_values = &outputs.to_owned().svd(false, false).singular_values;
        let pinv = outputs
            .to_owned()
            .pseudo_inverse(0.)
            .map_err(|_| err_msg("Taking the pinv of the output matrix failed"))?;
        let normalized_cov_params = &pinv * &pinv.transpose();
        let params = get_sum_of_products(&pinv, &inputs);
        if params.len() < 2 {
            bail!("Invalid parameter matrix");
        }
        Ok((params, singular_values.to_owned(), normalized_cov_params))
    }
}

/// A fitted regression model
///
#[derive(Debug)]
pub struct RegressionModel {
    pub data: Option<HashMap<String, Vec<f64>>>,
    pub formula: Option<String>,
    pub parameters: Vec<f64>,
    pub se: Vec<f64>,
    pub ssr: f64,
    pub rsquared: f64,
    pub rsquared_adj: f64,
    pub pvalues: Vec<f64>,
    pub residuals: Vec<f64>,
}
impl RegressionModel {
    pub fn try_from_regression_parameters(
        inputs: &RowDVector<f64>,
        outputs: &DMatrix<f64>,
        parameters: &DMatrix<f64>,
        singular_values: &DVector<f64>,
        normalized_cov_params: &DMatrix<f64>,
        data: HashMap<String, Vec<f64>>,
        formula: String,
    ) -> Result<Self, Error> {
        let diag = DMatrix::from_diagonal(&singular_values);
        let rank = &diag.rank(0.0);
        let input_vec: Vec<_> = inputs.iter().cloned().collect();
        let input_matrix = DMatrix::from_vec(inputs.len(), 1, input_vec);
        let residuals = &input_matrix - (outputs * parameters.to_owned());
        let ssr = residuals.dot(&residuals);
        let p = outputs.ncols() - 1;
        let n = inputs.ncols();
        let scale = ssr / ((n - p) as f64);
        let cov_params = normalized_cov_params.to_owned() * scale;
        let se = get_bse_from_cov_params(&cov_params)?;
        let centered_input_matrix = subtract_value_from_matrix(&input_matrix, input_matrix.mean());
        let centered_tss = &centered_input_matrix.dot(&centered_input_matrix);
        let rsquared = 1. - (ssr / centered_tss);
        let df_resid = n - rank;
        let rsquared_adj = 1. - ((n - 1) as f64 / df_resid as f64 * (1. - rsquared));
        let tvalues: Vec<_> = matrix_as_vec(parameters)
            .iter()
            .zip(matrix_as_vec(&se))
            .map(|(x, y)| x / y)
            .collect();
        let students_t = StudentsT::new(0.0, 1.0, df_resid as f64)?;
        let pvalues: Vec<_> = tvalues
            .iter()
            .cloned()
            .map(|x| (1. - students_t.cdf(x)) * 2.)
            .collect();
        // Convert these from interal Matrix types to user facing Vecs
        let parameters: Vec<_> = parameters.iter().cloned().collect();
        let se: Vec<_> = se.iter().cloned().collect();
        let residuals: Vec<_> = residuals.iter().cloned().collect();
        Ok(Self {
            data: Some(data),
            formula: Some(formula),
            parameters,
            se,
            ssr,
            rsquared,
            rsquared_adj,
            pvalues,
            residuals,
        })
    }
}

/// Performs a linear regression.
///
/// Peforms a ordinary least squared linear regression using the pseudo inverse method to solve the linear system.
/// This method supports multiple linear regression.
/// If successful it returns a `Vec` of the form `vec![intercept, slope1, slope2, ...]`.
///
pub fn ols_pinv(inputs: &RowDVector<f64>, outputs: &DMatrix<f64>) -> Result<Vec<f64>, Error> {
    let singular_values = &outputs.to_owned().svd(false, false).singular_values;
    let diag = DMatrix::from_diagonal(&singular_values);
    let rank = &diag.rank(0.0);
    let pinv = outputs
        .to_owned()
        .pseudo_inverse(0.)
        .map_err(|_| err_msg("Taking the pinv of the output matrix failed"))?;
    let normalized_cov_params = &pinv * &pinv.transpose();
    let params = get_sum_of_products(&pinv, &inputs);
    if params.len() < 2 {
        bail!("Invalid parameter matrix");
    }
    let result: Vec<f64> = params.iter().cloned().collect();
    let input_vec: Vec<_> = inputs.iter().cloned().collect();
    let input_matrix = DMatrix::from_vec(inputs.len(), 1, input_vec);
    let residuals = &input_matrix - (outputs * params.to_owned());
    let ssr = residuals.dot(&residuals);
    let p = outputs.ncols() - 1;
    let n = inputs.ncols();
    let scale = ssr / ((n - p) as f64);
    let cov_params = normalized_cov_params.to_owned() * scale;
    let bse = get_bse_from_cov_params(&cov_params)?;
    let centered_input_matrix = subtract_value_from_matrix(&input_matrix, input_matrix.mean());
    let centered_tss = &centered_input_matrix.dot(&centered_input_matrix);
    let rsquared = 1. - (ssr / centered_tss);
    let df_resid = n - rank;
    let _rsquared_adj = 1. - ((n - 1) as f64 / df_resid as f64 * (1. - rsquared));
    let tvalues: Vec<_> = matrix_as_vec(&params)
        .iter()
        .zip(matrix_as_vec(&bse))
        .map(|(x, y)| x / y)
        .collect();
    let students_t = StudentsT::new(0.0, 1.0, df_resid as f64)?;
    let _pvalues: Vec<_> = tvalues
        .iter()
        .cloned()
        .map(|x| (1. - students_t.cdf(x)) * 2.)
        .collect();
    Ok(result)
}
fn matrix_as_vec(matrix: &DMatrix<f64>) -> Vec<f64> {
    let mut vector = Vec::new();
    for row_index in 0..matrix.nrows() {
        let row = matrix.row(row_index);
        for i in row.iter() {
            vector.push(*i);
        }
    }
    vector
}
fn subtract_value_from_matrix(matrix: &DMatrix<f64>, value: f64) -> DMatrix<f64> {
    let mut v = Vec::new();
    for row_index in 0..matrix.nrows() {
        let row = matrix.row(row_index);
        for i in row.iter().map(|i| i - value) {
            v.push(i);
        }
    }
    DMatrix::from_vec(matrix.nrows(), matrix.ncols(), v)
}
fn get_bse_from_cov_params(matrix: &DMatrix<f64>) -> Result<DMatrix<f64>, Error> {
    let mut v = Vec::new();
    for row_index in 0..matrix.ncols() {
        let row = matrix.row(row_index);
        if row_index > row.len() {
            bail!("Matrix is not square");
        }
        v.push(row[row_index].sqrt());
    }
    Ok(DMatrix::from_vec(matrix.ncols(), 1, v))
}

/// Performs a linear regression.
///
/// Peforms a ordinary least squared linear regression using the QR method to solve the linear system.
/// This method does not support multile linear regression.
/// If successful it returns a tuple of the form `(intercept, slope)`.
///
pub fn ols_qr(inputs: &RowDVector<f64>, outputs: &DMatrix<f64>) -> Result<(f64, f64), Error> {
    let qr = outputs.to_owned().qr();
    let (q, r) = (qr.q(), qr.r());
    let _normalized_cov_params = &r.tr_mul(&r).pseudo_inverse(0.);
    let _singular_values = q.to_owned().svd(false, false);
    let effects = get_sum_of_products(&q.transpose(), &inputs);
    let result = r
        .qr()
        .solve(&effects)
        .ok_or_else(|| err_msg("Solving failed"))?;
    if result.len() < 2 {
        bail!("Invalid result matrix");
    }
    let intercept = result[0];
    let slope = result[1];
    Ok((slope, intercept))
}
fn get_sum_of_products(matrix: &DMatrix<f64>, vector: &RowDVector<f64>) -> DMatrix<f64> {
    let mut v: Vec<f64> = Vec::new();
    for row_index in 0..matrix.nrows() {
        let row = matrix.row(row_index);
        let mut sum = 0.;
        for (x, y) in row.iter().zip(vector.iter()) {
            sum += x * y;
        }
        v.push(sum);
    }
    DMatrix::from_vec(matrix.nrows(), 1, v)
}
#[cfg(test)]
mod tests {
    use super::*;
    use math::round;
    #[test]
    fn test_pinv_with_formula_builder() {
        use std::collections::HashMap;
        let inputs = vec![1., 3., 4., 5., 2., 3., 4.];
        let outputs1 = vec![1., 2., 3., 4., 5., 6., 7.];
        let outputs2 = vec![7., 6., 5., 4., 3., 2., 1.];
        let mut data = HashMap::new();
        data.insert("Y".to_string(), inputs);
        data.insert("X1".to_string(), outputs1);
        data.insert("X2".to_string(), outputs2);
        let regression = FormulaRegressionBuilder::new()
            .data(&data)
            .formula("Y ~ X1 + X2")
            .fit()
            .expect("Fitting model failed");

        let model_parameters = vec![0.09523809523809511, 0.5059523809523809, 0.25595238095238104];
        let se = vec![0.015457637291218271, 0.1417242813072997, 0.1417242813072997];
        let ssr = 9.107142857142858;
        let rsquared = 0.16118421052631582;
        let rsquared_adj = -0.006578947368421018;
        let pvalues = vec![
            0.0016390312044176625,
            0.01604408370984789,
            0.13074580446389206,
        ];
        let residuals = vec![
            -1.3928571428571432,
            0.35714285714285676,
            1.1071428571428568,
            1.8571428571428572,
            -1.3928571428571428,
            -0.6428571428571423,
            0.10714285714285765,
        ];
        assert_eq!(regression.parameters, model_parameters);
        assert_eq!(regression.se, se);
        assert_eq!(regression.ssr, ssr);
        assert_eq!(regression.rsquared, rsquared);
        assert_eq!(regression.rsquared_adj, rsquared_adj);
        assert_eq!(regression.pvalues, pvalues);
        assert_eq!(regression.residuals, residuals);
    }
    #[test]
    fn test_ols_qr() {
        let inputs = RowDVector::from_vec(vec![1., 3., 4., 5., 2., 3., 4.]);
        #[rustfmt::skip]
        let outputs = DMatrix::from_vec(7,2,
                                        vec![
                                        1., 1., 1., 1., 1., 1., 1.,
                                        1., 2., 3., 4., 5., 6., 7.]);
        let (intercept, slope) = ols_qr(&inputs, &outputs).expect("Solving failed!");
        let intercept = round::half_up(intercept, 2);
        let slope = round::half_up(slope, 8);
        assert_eq!((intercept, slope), (0.25, 2.14285714));
    }
    #[test]
    fn test_ols_pinv_single_regession() {
        let inputs = RowDVector::from_vec(vec![1., 3., 4., 5., 2., 3., 4.]);
        #[rustfmt::skip]
        let outputs = DMatrix::from_vec(7,2,
                                        vec![
                                        1., 1., 1., 1., 1., 1., 1.,
                                        1., 2., 3., 4., 5., 6., 7.]);
        let params = ols_pinv(&inputs, &outputs).expect("Solving failed!");
        let intercept = round::half_up(params[0], 8);
        let slope = round::half_up(params[1], 2);
        assert_eq!((intercept, slope), (2.14285714, 0.25));
    }
    #[test]
    fn test_ols_pinv_multiple_regression() {
        let inputs = RowDVector::from_vec(vec![1., 3., 4., 5., 2., 3., 4.]);
        #[rustfmt::skip]
        let outputs = DMatrix::from_vec(7,3,
                                        vec![
                                        1., 1., 1., 1., 1., 1., 1.,
                                        1., 2., 3., 4., 5., 6., 7.,
                                        7., 6., 5., 4., 3., 2., 1.]);
        let params = ols_pinv(&inputs, &outputs).expect("Solving failed!");
        let intercept = round::half_up(params[0], 8);
        let slope = round::half_up(params[1], 8);
        let slope2 = round::half_up(params[2], 8);
        assert_eq!(
            (intercept, slope, slope2),
            (0.0952381, 0.50595238, 0.25595238)
        );
    }
}
