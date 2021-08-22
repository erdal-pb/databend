// Copyright 2020 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::alloc::Layout;

use bumpalo::Bump;
use common_datablocks::DataBlock;
use common_datablocks::HashMethod;
use common_datavalues::arrays::ArrayBuilder;
use common_datavalues::arrays::BinaryArrayBuilder;
use common_datavalues::arrays::PrimitiveArrayBuilder;
use common_datavalues::prelude::IntoSeries;
use common_datavalues::prelude::Series;
use common_datavalues::DFNumericType;
use common_datavalues::DataSchemaRef;
use common_datavalues::DataSchemaRefExt;
use common_exception::Result;
use common_functions::aggregates::get_layout_offsets;
use common_functions::aggregates::AggregateFunctionRef;
use common_functions::aggregates::StateAddr;
use common_infallible::RwLock;
use common_io::prelude::BytesMut;
use common_planners::Expression;
use common_streams::DataBlockStream;
use common_streams::SendableDataBlockStream;
use futures::StreamExt;

use crate::common::DefaultHashTableEntity;
use crate::common::DefaultHasher;
use crate::common::HashMap;
use crate::common::HashTableEntity;
use crate::common::KeyHasher;
use crate::pipelines::transforms::aggregator::aggregator_params::{AggregatorParamsRef, AggregatorParams};
use crate::pipelines::transforms::aggregator::aggregator_area::AggregatorArea;
use common_datavalues::columns::DataColumn;

pub struct Aggregator<Method: HashMethod> {
    method: Method,
    params: AggregatorParamsRef,
    layout: Layout,
    offsets_aggregate_states: Vec<usize>,
}

impl<Method: HashMethod> Aggregator<Method> where
    DefaultHasher<Method::HashKey>: KeyHasher<Method::HashKey>,
    DefaultHashTableEntity<Method::HashKey, usize>: HashTableEntity<Method::HashKey>,
{
    pub fn create(
        method: Method,
        aggr_exprs: &[Expression],
        schema: DataSchemaRef,
    ) -> Result<Aggregator<Method>> {
        let aggregator_params = AggregatorParams::try_create(schema, aggr_exprs)?;
        // let aggregator_area = AggregatorArea::try_create(&aggregator_params)?;

        let aggregate_functions = &aggregator_params.aggregate_functions;
        let (states_layout, states_offsets) = unsafe { get_layout_offsets(aggregate_functions) };

        Ok(Aggregator {
            method,
            params: aggregator_params,
            layout: states_layout,
            offsets_aggregate_states: states_offsets,
        })
    }

    pub async fn aggregate(
        &self,
        group_cols: Vec<String>,
        mut stream: SendableDataBlockStream,
    ) -> Result<RwLock<(HashMap<Method::HashKey, usize>, Bump)>> {
        let groups_locker = RwLock::new((HashMap::<Method::HashKey, usize>::create(), Bump::new()));

        let hash_method = &self.method;
        let aggregator_params = self.params.as_ref();

        let aggr_len = aggregator_params.aggregate_functions.len();
        let func = &aggregator_params.aggregate_functions;

        let layout = self.layout;
        let offsets_aggregate_states = &self.offsets_aggregate_states;

        while let Some(block) = stream.next().await {
            let block = block?;

            // 1.1 and 1.2.
            let group_columns = Self::group_columns(&group_cols, &block)?;
            let aggregate_args_columns = self.aggregate_arguments_column(&block)?;

            let mut places = Vec::with_capacity(block.num_rows());
            let group_keys = hash_method.build_keys(&group_columns, block.num_rows())?;
            let mut groups = groups_locker.write();
            {
                for group_key in group_keys.iter() {
                    let mut inserted = true;
                    let entity = groups.0.insert_key(group_key, &mut inserted);

                    match inserted {
                        true => {
                            if aggr_len == 0 {
                                entity.set_value(0);
                            } else {
                                let place: StateAddr = groups.1.alloc_layout(layout).into();
                                for idx in 0..aggr_len {
                                    let aggr_state = offsets_aggregate_states[idx];
                                    let aggr_state_place = place.next(aggr_state);
                                    func[idx].init_state(aggr_state_place);
                                }
                                places.push(place);
                                entity.set_value(place.addr());
                            }
                        }
                        false => {
                            let place: StateAddr = (*entity.get_value()).into();
                            places.push(place);
                        }
                    }
                }
            }

            {
                // this can benificial for the case of dereferencing
                let aggr_arg_columns_slice = &aggregate_args_columns;

                for ((idx, func), args) in
                func.iter().enumerate().zip(aggr_arg_columns_slice.iter())
                {
                    func.accumulate_keys(
                        &places,
                        offsets_aggregate_states[idx],
                        args,
                        block.num_rows(),
                    )?;
                }
            }
        }
        Ok(groups_locker)
    }

    #[inline(always)]
    fn aggregate_arguments_column(&self, block: &DataBlock) -> Result<Vec<Vec<Series>>> {
        let aggregator_params = self.params.as_ref();

        let aggregate_functions = &aggregator_params.aggregate_functions;
        let aggregate_functions_arguments = &aggregator_params.aggregate_functions_arguments_name;

        let mut aggregate_arguments_columns = Vec::with_capacity(aggregate_functions.len());
        for index in 0..aggregate_functions.len() {
            let function_arguments = &aggregate_functions_arguments[index];

            let mut function_arguments_column = Vec::with_capacity(function_arguments.len());
            for argument_index in 0..function_arguments.len() {
                let argument_name = &function_arguments[argument_index];
                let argument_column = block.try_column_by_name(argument_name)?;
                function_arguments_column.push(argument_column.to_array()?);
            }

            aggregate_arguments_columns.push(function_arguments_column);
        }

        Ok(aggregate_arguments_columns)
    }

    #[inline(always)]
    fn group_columns<'a>(names: &[String], block: &'a DataBlock) -> Result<Vec<&'a DataColumn>> {
        names
            .iter()
            .map(|column_name| block.try_column_by_name(column_name))
            .collect::<Result<Vec<&DataColumn>>>()
    }

    pub fn aggregate_finalized<T: DFNumericType>(
        &self,
        groups: &HashMap<T::Native, usize>,
        schema: DataSchemaRef,
    ) -> Result<SendableDataBlockStream>
        where
            DefaultHasher<T::Native>: KeyHasher<T::Native>,
            DefaultHashTableEntity<T::Native, usize>: HashTableEntity<T::Native>,
    {
        if groups.is_empty() {
            return Ok(Box::pin(DataBlockStream::create(
                DataSchemaRefExt::create(vec![]),
                None,
                vec![],
            )));
        }

        let aggregator_params = self.params.as_ref();
        let funcs = &aggregator_params.aggregate_functions;
        let aggr_len = funcs.len();
        let offsets_aggregate_states = &self.offsets_aggregate_states;

        // Builders.
        let mut state_builders: Vec<BinaryArrayBuilder> = (0..aggr_len)
            .map(|_| BinaryArrayBuilder::with_capacity(groups.len() * 4))
            .collect();

        let mut group_key_builder = PrimitiveArrayBuilder::<T>::with_capacity(groups.len());

        let mut bytes = BytesMut::new();
        for group_entity in groups.iter() {
            let place: StateAddr = (*group_entity.get_value()).into();

            for (idx, func) in funcs.iter().enumerate() {
                let arg_place = place.next(offsets_aggregate_states[idx]);
                func.serialize(arg_place, &mut bytes)?;
                state_builders[idx].append_value(&bytes[..]);
                bytes.clear();
            }

            group_key_builder.append_value(*(group_entity.get_key()));
        }

        let mut columns: Vec<Series> = Vec::with_capacity(schema.fields().len());
        for mut builder in state_builders {
            columns.push(builder.finish().into_series());
        }
        let array = group_key_builder.finish();
        columns.push(array.array.into_series());

        let block = DataBlock::create_by_array(schema.clone(), columns);
        Ok(Box::pin(DataBlockStream::create(schema, None, vec![block])))
    }
}
